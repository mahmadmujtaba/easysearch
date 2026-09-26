//! In-place self-updates over HTTPS — no `.deb`/`.rpm`, no package manager.
//!
//! A release publishes two files next to the binaries:
//!
//! ```text
//! manifest.json       the signed description of a release (below)
//! manifest.json.sig   a detached Ed25519 signature over the exact bytes of
//!                     manifest.json, hex-encoded
//! ```
//!
//! The trust chain is deliberately small and **fails closed**:
//!
//! 1. `manifest.json` is fetched over **HTTPS only** and its detached Ed25519
//!    signature is verified against the release public key compiled into the
//!    binary ([`RELEASE_PUBLIC_KEY_HEX`]). No key, no signature, or a bad
//!    signature ⇒ the update is refused.
//! 2. The manifest lists each asset's **SHA-256**; every download is hashed and
//!    must match *before* anything is written into place.
//! 3. Assets are installed by copying beside the destination and `rename()`-ing
//!    over it: atomic on one filesystem, and safe while the old binary is still
//!    running (the running inode stays valid until the process exits).
//!
//! Transport is `curl`, pinned to `https`, so the engine carries no TLS stack;
//! all security-relevant logic — URL scheme, signature, hash, atomic install —
//! lives here and is unit-tested against an in-memory [`Fetcher`].

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Manifest URL used unless overridden (see [`ENV_MANIFEST_URL`]).
///
/// Convention: this is a *stable* URL that always points at the newest release's
/// manifest (a GitHub `releases/latest/download/…` asset, or any HTTPS host).
pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/mahmadmujtaba/easysearch/releases/latest/download/manifest.json";

/// Ed25519 release public key (32 bytes, hex). Signatures produced by the
/// matching private key are the only ones accepted.
///
/// The private half is **not** in this repository; see `docs/updates.md`.
pub const RELEASE_PUBLIC_KEY_HEX: &str =
    "5026f1bc8609fc7eb22d7cbba4caec2831532e1717ffc147a43bf5d5ed1af26b";

/// Override the manifest URL (useful for self-hosted mirrors and tests).
pub const ENV_MANIFEST_URL: &str = "EASYSEARCH_UPDATE_URL";
/// Override the trusted public key (hex). Setting this is equivalent to
/// trusting that key completely — only do it deliberately.
pub const ENV_PUBLIC_KEY: &str = "EASYSEARCH_UPDATE_PUBKEY";
/// Override the directory the new binaries are installed into.
pub const ENV_INSTALL_DIR: &str = "EASYSEARCH_UPDATE_DIR";
/// Set to a non-empty value to disable update checks entirely.
pub const ENV_DISABLE: &str = "EASYSEARCH_NO_UPDATE";

/// Binaries a normal install may contain, next to the running executable.
pub const KNOWN_BINARIES: &[&str] = &[
    "easysearch",
    "easysearch-cli",
    "easysearch-gui",
    "easysearch-daemon",
];

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// One downloadable file: a raw, already-executable binary.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Asset {
    /// File name **as installed** (e.g. `easysearch`).
    pub name: String,
    /// HTTPS URL of the binary.
    pub url: String,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: String,
    /// Size in bytes (used for progress and as a sanity check; 0 = unknown).
    #[serde(default)]
    pub size: u64,
    /// Optional target triple (`x86_64-unknown-linux-gnu`); absent = any.
    #[serde(default)]
    pub target: Option<String>,
}

/// A signed release description.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    /// New version, e.g. `0.15.0` (leading `v` is tolerated).
    pub version: String,
    #[serde(default)]
    pub released: Option<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// A newer release that is available for this machine.
#[derive(Clone, Debug)]
pub struct Available {
    pub version: String,
    pub notes: String,
    /// The whole manifest, so installation never refetches it.
    pub manifest: Manifest,
}

/// Progress stages reported during [`Updater::install`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Downloading,
    Verifying,
    Installing,
}

/// What an installation did.
#[derive(Clone, Debug)]
pub struct InstallReport {
    pub version: String,
    /// Absolute paths that were replaced.
    pub files: Vec<PathBuf>,
    /// Restart the app (and its daemon) to run the new code.
    pub restart_required: bool,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum UpdateError {
    /// A URL that is not `https://`.
    Insecure(String),
    /// Network/transport failure (message from the fetcher).
    Fetch(String),
    /// `manifest.json` was not valid JSON/shape.
    BadManifest(String),
    /// No trusted public key configured ⇒ refuse (fail closed).
    NoKey,
    /// The configured public key is not a valid 32-byte Ed25519 key.
    BadKey,
    /// The manifest signature did not verify.
    BadSignature,
    /// A downloaded asset's SHA-256 differed from the manifest.
    HashMismatch {
        name: String,
        expected: String,
        actual: String,
    },
    /// A downloaded asset's size differed from the manifest.
    SizeMismatch {
        name: String,
        expected: u64,
        actual: u64,
    },
    /// The manifest has no asset for this machine / install.
    NoAsset(String),
    /// Writing the new binary failed (permissions, read-only install dir, …).
    Install { path: PathBuf, message: String },
    /// Local I/O error.
    Io(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateError::Insecure(u) => write!(f, "refusing non-HTTPS URL: {u}"),
            UpdateError::Fetch(m) => write!(f, "download failed: {m}"),
            UpdateError::BadManifest(m) => write!(f, "invalid release manifest: {m}"),
            UpdateError::NoKey => write!(
                f,
                "no release signing key is trusted, so updates are disabled"
            ),
            UpdateError::BadKey => write!(f, "the trusted release key is malformed"),
            UpdateError::BadSignature => write!(
                f,
                "release manifest signature is invalid — refusing the update"
            ),
            UpdateError::HashMismatch {
                name,
                expected,
                actual,
            } => write!(
                f,
                "checksum mismatch for {name}: manifest says {expected}, download is {actual}"
            ),
            UpdateError::SizeMismatch {
                name,
                expected,
                actual,
            } => write!(
                f,
                "size mismatch for {name}: manifest says {expected} bytes, download is {actual}"
            ),
            UpdateError::NoAsset(t) => {
                write!(f, "the release has no asset for {t} / this install")
            }
            UpdateError::Install { path, message } => {
                write!(f, "cannot install to {}: {message}", path.display())
            }
            UpdateError::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for UpdateError {}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// Minimal HTTP transport. Kept as a trait so the security logic can be tested
/// without a network (and so a different transport can be swapped in later).
pub trait Fetcher {
    /// `GET url`, returning the whole body.
    fn get(&self, url: &str) -> Result<Vec<u8>, String>;

    /// Stream `url` into `dest`, calling `on_progress(written, total_hint)` as
    /// bytes arrive. Returns the number of bytes written.
    fn download(
        &self,
        url: &str,
        dest: &Path,
        total_hint: u64,
        on_progress: &mut dyn FnMut(u64, u64),
    ) -> Result<u64, String>;
}

/// HTTPS-only transport backed by the system `curl`.
#[derive(Clone, Debug, Default)]
pub struct CurlFetcher {
    /// The `curl` program (defaults to `curl` from `PATH`).
    pub program: Option<String>,
    /// Whole-request timeout in seconds.
    pub timeout_secs: u64,
}

impl CurlFetcher {
    pub fn new() -> CurlFetcher {
        CurlFetcher {
            program: None,
            timeout_secs: 300,
        }
    }

    fn program(&self) -> &str {
        self.program.as_deref().unwrap_or("curl")
    }

    /// Flags that pin the transfer to HTTPS (including across redirects) and
    /// keep the output clean.
    fn common_args(&self) -> Vec<String> {
        vec![
            "--fail".into(),
            "--silent".into(),
            "--show-error".into(),
            "--location".into(),
            "--max-redirs".into(),
            "5".into(),
            // Refuse anything that is not https, before and after redirects.
            "--proto".into(),
            "=https".into(),
            "--proto-redir".into(),
            "=https".into(),
            "--tlsv1.2".into(),
            "--no-progress-meter".into(),
            "--connect-timeout".into(),
            "15".into(),
            "--max-time".into(),
            self.timeout_secs.to_string(),
            "--user-agent".into(),
            format!("easysearch/{}", env!("CARGO_PKG_VERSION")),
        ]
    }
}

impl Fetcher for CurlFetcher {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        ensure_https(url).map_err(|e| e.to_string())?;
        let out = Command::new(self.program())
            .args(self.common_args())
            .arg(url)
            .output()
            .map_err(|e| format!("cannot run {}: {e}", self.program()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("{url}: {}", err.trim()));
        }
        Ok(out.stdout)
    }

    fn download(
        &self,
        url: &str,
        dest: &Path,
        total_hint: u64,
        on_progress: &mut dyn FnMut(u64, u64),
    ) -> Result<u64, String> {
        ensure_https(url).map_err(|e| e.to_string())?;
        let mut child = Command::new(self.program())
            .args(self.common_args())
            .arg(url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", self.program()))?;

        // Drain stderr on its own thread so a chatty failure can never fill the
        // pipe and deadlock the transfer.
        let mut stderr = child.stderr.take();
        let err_thread = stderr.take().map(|mut e| {
            std::thread::spawn(move || {
                let mut s = String::new();
                let _ = e.read_to_string(&mut s);
                s
            })
        });

        let mut out = child.stdout.take().ok_or("curl produced no stdout")?;
        let mut file = std::fs::File::create(dest).map_err(|e| format!("create {dest:?}: {e}"))?;
        let mut buf = [0u8; 64 * 1024];
        let mut written: u64 = 0;
        loop {
            let n = out.read(&mut buf).map_err(|e| format!("read from curl: {e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| format!("write {dest:?}: {e}"))?;
            written += n as u64;
            on_progress(written, total_hint);
        }
        file.flush().ok();
        drop(file);

        let status = child.wait().map_err(|e| format!("wait for curl: {e}"))?;
        if !status.success() {
            let msg = err_thread
                .and_then(|t| t.join().ok())
                .unwrap_or_default();
            return Err(format!("{url}: {}", msg.trim()));
        }
        Ok(written)
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Everything the updater needs to know about *this* machine and install.
#[derive(Clone, Debug)]
pub struct UpdateConfig {
    /// HTTPS URL of `manifest.json`.
    pub manifest_url: String,
    /// Trusted Ed25519 public key (hex). `None` ⇒ use the compiled-in key.
    pub public_key_hex: Option<String>,
    /// The version currently running (compared against the manifest).
    pub current_version: String,
    /// Target triple of this machine.
    pub target: String,
    /// Directory the binaries live in. `None` ⇒ the running executable's dir.
    pub install_dir: Option<PathBuf>,
    /// Which asset names to install (e.g. only the binaries that are installed).
    pub install_names: Vec<String>,
    /// Scratch directory for staged downloads.
    pub cache_dir: PathBuf,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        let install_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        UpdateConfig {
            manifest_url: DEFAULT_MANIFEST_URL.to_string(),
            public_key_hex: None,
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            target: format!("{}-unknown-linux-gnu", std::env::consts::ARCH),
            install_dir: install_dir.clone(),
            install_names: install_dir
                .as_deref()
                .map(installed_binaries)
                .unwrap_or_default(),
            cache_dir: default_cache_dir().join("update"),
        }
    }
}

impl UpdateConfig {
    /// Apply the `EASYSEARCH_UPDATE_*` environment overrides (used by the CLI
    /// and by self-hosted installs). An empty [`ENV_MANIFEST_URL`] or
    /// [`ENV_PUBLIC_KEY`] means "leave as-is".
    pub fn with_env(mut self) -> UpdateConfig {
        if let Some(url) = std::env::var(ENV_MANIFEST_URL)
            .ok()
            .filter(|s| !s.trim().is_empty())
        {
            self.manifest_url = url.trim().to_string();
        }
        if let Some(key) = std::env::var(ENV_PUBLIC_KEY)
            .ok()
            .filter(|s| !s.trim().is_empty())
        {
            self.public_key_hex = Some(key.trim().to_string());
        }
        if let Some(dir) = std::env::var(ENV_INSTALL_DIR)
            .ok()
            .filter(|s| !s.trim().is_empty())
        {
            let dir = PathBuf::from(dir.trim());
            self.install_names = installed_binaries(&dir);
            self.install_dir = Some(dir);
        }
        self
    }

    /// True when update checks are switched off by the environment.
    pub fn disabled() -> bool {
        std::env::var(ENV_DISABLE).is_ok_and(|v| !v.trim().is_empty())
    }

    /// Effective trusted key (configured, else compiled in).
    pub fn public_key(&self) -> Option<&str> {
        match &self.public_key_hex {
            Some(k) => Some(k.as_str()),
            None => Some(RELEASE_PUBLIC_KEY_HEX),
        }
    }
}

/// `$XDG_CACHE_HOME/easysearch` (or `~/.cache/easysearch`).
pub fn default_cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".cache"))
                .unwrap_or_else(|| PathBuf::from("."))
        });
    base.join("easysearch")
}

/// Names of the known binaries that exist in `dir`.
pub fn installed_binaries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    // Always include the running executable, even if it lives elsewhere.
    if let Ok(exe) = std::env::current_exe()
        && let Some(n) = exe.file_name().and_then(|s| s.to_str())
        && !names.iter().any(|x| x == n)
    {
        names.push(n.to_string());
    }
    for k in KNOWN_BINARIES {
        if dir.join(k).exists() && !names.iter().any(|x| x == k) {
            names.push((*k).to_string());
        }
    }
    names
}

// ---------------------------------------------------------------------------
// The updater
// ---------------------------------------------------------------------------

pub struct Updater {
    config: UpdateConfig,
}

impl Updater {
    pub fn new(config: UpdateConfig) -> Updater {
        Updater { config }
    }

    pub fn config(&self) -> &UpdateConfig {
        &self.config
    }

    fn install_dir(&self) -> PathBuf {
        self.config
            .install_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn wants(&self, asset: &Asset) -> bool {
        self.config.install_names.iter().any(|n| n == &asset.name)
            && asset
                .target
                .as_deref()
                .is_none_or(|t| t == self.config.target)
    }

    /// Fetch and verify the manifest; return `Some(available)` when it is newer
    /// than the running version, `None` when already up to date.
    pub fn check(&self, fetcher: &dyn Fetcher) -> Result<Option<Available>, UpdateError> {
        let manifest = self.fetch_manifest(fetcher)?;
        if !is_newer(&manifest.version, &self.config.current_version) {
            return Ok(None);
        }
        if !manifest.assets.iter().any(|a| self.wants(a)) {
            return Err(UpdateError::NoAsset(self.config.target.clone()));
        }
        Ok(Some(Available {
            version: manifest.version.clone(),
            notes: manifest.notes.clone(),
            manifest,
        }))
    }

    /// Fetch, verify and install a release, then report what changed.
    ///
    /// All assets are downloaded and verified **before** any of them is put in
    /// place, so a failed download never leaves a half-updated install.
    pub fn install(
        &self,
        available: &Available,
        fetcher: &dyn Fetcher,
        on_progress: &mut dyn FnMut(Stage, u64, u64),
    ) -> Result<InstallReport, UpdateError> {
        let targets: Vec<&Asset> = available
            .manifest
            .assets
            .iter()
            .filter(|a| self.wants(a))
            .collect();
        if targets.is_empty() {
            return Err(UpdateError::NoAsset(self.config.target.clone()));
        }

        std::fs::create_dir_all(&self.config.cache_dir)
            .map_err(|e| UpdateError::Io(format!("create {:?}: {e}", self.config.cache_dir)))?;

        // Phase 1 — stage and verify everything.
        let mut staged: Vec<(PathBuf, &Asset)> = Vec::new();
        for asset in &targets {
            ensure_https(&asset.url)?;
            let path = self.config.cache_dir.join(format!("{}.staged", asset.name));
            on_progress(Stage::Downloading, 0, asset.size);
            let written = fetcher
                .download(&asset.url, &path, asset.size, &mut |done, total| {
                    on_progress(Stage::Downloading, done, total)
                })
                .map_err(UpdateError::Fetch)?;

            on_progress(Stage::Verifying, 0, written);
            let actual = sha256_file(&path)?;
            if !actual.eq_ignore_ascii_case(asset.sha256.trim()) {
                let _ = std::fs::remove_file(&path);
                return Err(UpdateError::HashMismatch {
                    name: asset.name.clone(),
                    expected: asset.sha256.trim().to_string(),
                    actual,
                });
            }
            if asset.size != 0 && written != asset.size {
                let _ = std::fs::remove_file(&path);
                return Err(UpdateError::SizeMismatch {
                    name: asset.name.clone(),
                    expected: asset.size,
                    actual: written,
                });
            }
            staged.push((path, asset));
        }

        // Phase 2 — atomically put each verified file in place.
        let dir = self.install_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| UpdateError::Io(format!("create {:?}: {e}", dir)))?;
        let mut files = Vec::new();
        for (path, asset) in staged {
            on_progress(Stage::Installing, 0, 1);
            let dest = dir.join(&asset.name);
            replace_file(&path, &dest).map_err(|e| UpdateError::Install {
                path: dest.clone(),
                message: e.to_string(),
            })?;
            let _ = std::fs::remove_file(&path);
            files.push(dest);
        }

        Ok(InstallReport {
            version: available.version.clone(),
            files,
            restart_required: true,
        })
    }

    fn fetch_manifest(&self, fetcher: &dyn Fetcher) -> Result<Manifest, UpdateError> {
        ensure_https(&self.config.manifest_url)?;
        let raw = fetcher
            .get(&self.config.manifest_url)
            .map_err(UpdateError::Fetch)?;

        let sig_url = format!("{}.sig", self.config.manifest_url);
        ensure_https(&sig_url)?;
        let sig_raw = fetcher.get(&sig_url).map_err(UpdateError::Fetch)?;
        let sig_hex = String::from_utf8_lossy(&sig_raw);
        let key = self.config.public_key().ok_or(UpdateError::NoKey)?;
        verify_signature(&raw, sig_hex.trim(), key)?;

        serde_json::from_slice(&raw).map_err(|e| UpdateError::BadManifest(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Verification primitives
// ---------------------------------------------------------------------------

/// Reject anything that is not an `https://` URL. The update path never talks
/// cleartext, so a downgrade is a hard error rather than a warning.
pub fn ensure_https(url: &str) -> Result<(), UpdateError> {
    if url.len() >= 8 && url[..8].eq_ignore_ascii_case("https://") {
        Ok(())
    } else {
        Err(UpdateError::Insecure(url.to_string()))
    }
}

/// Verify a detached Ed25519 signature (hex) over `message`.
pub fn verify_signature(
    message: &[u8],
    signature_hex: &str,
    public_key_hex: &str,
) -> Result<(), UpdateError> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let key_bytes = hex_decode(public_key_hex).map_err(|_| UpdateError::BadKey)?;
    let key_arr: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| UpdateError::BadKey)?;
    let key = VerifyingKey::from_bytes(&key_arr).map_err(|_| UpdateError::BadKey)?;

    let sig_bytes = hex_decode(signature_hex).map_err(|_| UpdateError::BadSignature)?;
    let sig_arr: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .map_err(|_| UpdateError::BadSignature)?;
    let signature = Signature::from_bytes(&sig_arr);

    key.verify(message, &signature)
        .map_err(|_| UpdateError::BadSignature)
}

/// Lowercase hex SHA-256 of a local file.
pub fn sha256_file(path: &Path) -> Result<String, UpdateError> {
    let mut file =
        std::fs::File::open(path).map_err(|e| UpdateError::Io(format!("open {path:?}: {e}")))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| UpdateError::Io(format!("read {path:?}: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

/// Copy `staged` beside `dest` and atomically `rename()` it into place, with the
/// executable bit set. Replacing a *running* binary this way is safe on Linux.
pub fn replace_file(staged: &Path, dest: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let dir = dest.parent().unwrap_or_else(|| Path::new("."));
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "binary".to_string());
    let tmp = dir.join(format!(".{name}.new"));

    std::fs::copy(staged, &tmp)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    if let Err(e) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Version ordering
// ---------------------------------------------------------------------------

/// True when `candidate` is strictly newer than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    compare_versions(candidate, current) == std::cmp::Ordering::Greater
}

/// Compare two dotted versions (`1.2.3`, `v1.2`, `1.2.0-beta.1`). A release
/// outranks a pre-release of the same numeric core.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let (na, pa) = parse_version(a);
    let (nb, pb) = parse_version(b);
    for i in 0..na.len().max(nb.len()) {
        match na
            .get(i)
            .copied()
            .unwrap_or(0)
            .cmp(&nb.get(i).copied().unwrap_or(0))
        {
            Ordering::Equal => {}
            other => return other,
        }
    }
    match (pa, pb) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => a.cmp(&b),
    }
}

fn parse_version(v: &str) -> (Vec<u64>, Option<String>) {
    let v = v.trim();
    let v = v.strip_prefix('v').unwrap_or(v);
    // Ignore build metadata (`+…`); keep the pre-release part.
    let v = v.split('+').next().unwrap_or(v);
    match v.split_once('-') {
        Some((core, pre)) => (core_numbers(core), Some(pre.to_string())),
        None => (core_numbers(v), None),
    }
}

fn core_numbers(core: &str) -> Vec<u64> {
    core.split('.').map(|p| p.trim().parse().unwrap_or(0)).collect()
}

// ---------------------------------------------------------------------------
// Hex
// ---------------------------------------------------------------------------

/// Lowercase hex encoding.
pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// Hex decoding; whitespace is ignored, odd length and non-hex are errors.
pub fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    let cleaned: Vec<u8> = s
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    if !cleaned.len().is_multiple_of(2) {
        return Err("odd-length hex".to_string());
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    for pair in cleaned.chunks(2) {
        let hi = hex_val(pair[0])?;
        let lo = hex_val(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_val(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("not a hex digit: {:?}", b as char)),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::collections::HashMap;

    /// In-memory transport: URL → body. No network, no filesystem beyond the
    /// staged download.
    struct MemFetcher {
        files: HashMap<String, Vec<u8>>,
    }

    impl MemFetcher {
        fn new() -> MemFetcher {
            MemFetcher {
                files: HashMap::new(),
            }
        }
        fn put(mut self, url: &str, body: impl Into<Vec<u8>>) -> MemFetcher {
            self.files.insert(url.to_string(), body.into());
            self
        }
    }

    impl Fetcher for MemFetcher {
        fn get(&self, url: &str) -> Result<Vec<u8>, String> {
            self.files
                .get(url)
                .cloned()
                .ok_or_else(|| format!("404 {url}"))
        }
        fn download(
            &self,
            url: &str,
            dest: &Path,
            _total_hint: u64,
            on_progress: &mut dyn FnMut(u64, u64),
        ) -> Result<u64, String> {
            let data = self.get(url)?;
            std::fs::write(dest, &data).map_err(|e| e.to_string())?;
            on_progress(data.len() as u64, data.len() as u64);
            Ok(data.len() as u64)
        }
    }

    /// A unique scratch directory per test.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "evfl-update-{tag}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SIGNING_SEED: [u8; 32] = [42u8; 32];

    fn keypair() -> (SigningKey, String) {
        let sk = SigningKey::from_bytes(&SIGNING_SEED);
        let pk_hex = hex_encode(sk.verifying_key().as_bytes());
        (sk, pk_hex)
    }

    /// Sign `manifest` and return `(manifest_bytes, sig_hex)`.
    fn sign(sk: &SigningKey, manifest: &Manifest) -> (Vec<u8>, String) {
        let bytes = serde_json::to_vec(manifest).unwrap();
        let sig = sk.sign(&bytes);
        (bytes, hex_encode(&sig.to_bytes()))
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        hex_encode(&h.finalize())
    }

    fn config(scratch: &Path, pk_hex: &str, names: &[&str], current: &str) -> UpdateConfig {
        UpdateConfig {
            manifest_url: "https://example.test/manifest.json".to_string(),
            public_key_hex: Some(pk_hex.to_string()),
            current_version: current.to_string(),
            target: "x86_64-unknown-linux-gnu".to_string(),
            install_dir: Some(scratch.join("bin")),
            install_names: names.iter().map(|s| s.to_string()).collect(),
            cache_dir: scratch.join("cache"),
        }
    }

    #[test]
    fn version_ordering() {
        assert!(is_newer("0.15.0", "0.14.1"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(is_newer("1.2.10", "1.2.9"));
        assert!(is_newer("1.2.0", "1.2.0-beta.1"));
        assert!(!is_newer("1.2.0-beta.1", "1.2.0"));
        assert!(!is_newer("1.2.0", "1.2.0"));
        assert!(!is_newer("1.1.9", "1.2.0"));
        assert_eq!(
            compare_versions("v0.15.0", "0.15.0"),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn hex_roundtrip_and_errors() {
        let bytes = [0x00u8, 0x0f, 0xa5, 0xff];
        assert_eq!(hex_encode(&bytes), "000fa5ff");
        assert_eq!(hex_decode("000fa5ff").unwrap(), bytes);
        assert_eq!(hex_decode("00 0f a5 ff").unwrap(), bytes); // whitespace tolerated
        assert!(hex_decode("abc").is_err()); // odd length
        assert!(hex_decode("zz").is_err()); // not hex
    }

    #[test]
    fn signature_verifies_and_tampering_is_rejected() {
        let (sk, pk) = keypair();
        let message = b"the quick brown fox";
        let sig = hex_encode(&sk.sign(message).to_bytes());

        assert!(verify_signature(message, &sig, &pk).is_ok());
        // A different message must not verify.
        assert!(matches!(
            verify_signature(b"a different message", &sig, &pk),
            Err(UpdateError::BadSignature)
        ));
        // A different key must not verify.
        let other_pk = hex_encode(SigningKey::from_bytes(&[9u8; 32]).verifying_key().as_bytes());
        assert!(matches!(
            verify_signature(message, &sig, &other_pk),
            Err(UpdateError::BadSignature)
        ));
        assert!(matches!(
            verify_signature(message, "not-hex", &pk),
            Err(UpdateError::BadSignature)
        ));
    }

    #[test]
    fn insecure_urls_are_refused() {
        assert!(matches!(
            ensure_https("http://example.test/x"),
            Err(UpdateError::Insecure(_))
        ));
        assert!(matches!(
            ensure_https("ftp://example.test/x"),
            Err(UpdateError::Insecure(_))
        ));
        assert!(ensure_https("https://example.test/x").is_ok());
        assert!(ensure_https("HTTPS://EXAMPLE.test/x").is_ok());
    }

    /// Build a signed manifest for one asset and a fetcher that serves it.
    fn release_fixture(
        scratch: &Path,
        version: &str,
        name: &str,
        payload: &[u8],
    ) -> (MemFetcher, SigningKey, String) {
        let (sk, pk) = keypair();
        let manifest = Manifest {
            version: version.to_string(),
            released: Some("2026-09-26".to_string()),
            notes: "Test release".to_string(),
            assets: vec![Asset {
                name: name.to_string(),
                url: format!("https://example.test/{name}"),
                sha256: sha256_hex(payload),
                size: payload.len() as u64,
                target: Some("x86_64-unknown-linux-gnu".to_string()),
            }],
        };
        let (bytes, sig_hex) = sign(&sk, &manifest);
        let fetcher = MemFetcher::new()
            .put("https://example.test/manifest.json", bytes)
            .put("https://example.test/manifest.json.sig", sig_hex)
            .put(&format!("https://example.test/{name}"), payload.to_vec());
        let _ = scratch;
        (fetcher, sk, pk)
    }

    #[test]
    fn check_reports_newer_release() {
        let scratch = Scratch::new("check");
        let (fetcher, _sk, pk) = release_fixture(scratch.path(), "0.15.0", "easysearch", b"NEW");
        let cfg = config(scratch.path(), &pk, &["easysearch"], "0.14.1");
        let up = Updater::new(cfg);

        let avail = up.check(&fetcher).unwrap().expect("a newer release");
        assert_eq!(avail.version, "0.15.0");
        assert_eq!(avail.notes, "Test release");
    }

    #[test]
    fn check_is_none_when_up_to_date() {
        let scratch = Scratch::new("uptodate");
        let (fetcher, _sk, pk) = release_fixture(scratch.path(), "0.14.1", "easysearch", b"x");
        let cfg = config(scratch.path(), &pk, &["easysearch"], "0.14.1");
        assert!(Updater::new(cfg).check(&fetcher).unwrap().is_none());
    }

    #[test]
    fn check_refuses_a_forged_manifest() {
        let scratch = Scratch::new("forged");
        let (fetcher, _sk, pk) = release_fixture(scratch.path(), "9.9.9", "easysearch", b"NEW");
        // Attacker swaps in their own manifest but cannot re-sign it.
        let attacker = SigningKey::from_bytes(&[1u8; 32]);
        let forged = Manifest {
            version: "9.9.9".to_string(),
            released: None,
            notes: "evil".to_string(),
            assets: vec![],
        };
        let (bytes, sig) = sign(&attacker, &forged);
        let fetcher = fetcher
            .put("https://example.test/manifest.json", bytes)
            .put("https://example.test/manifest.json.sig", sig);

        let cfg = config(scratch.path(), &pk, &["easysearch"], "0.14.1");
        assert!(matches!(
            Updater::new(cfg).check(&fetcher),
            Err(UpdateError::BadSignature)
        ));
    }

    #[test]
    fn check_requires_a_trusted_key() {
        let scratch = Scratch::new("nokey");
        let (fetcher, _sk, _pk) = release_fixture(scratch.path(), "0.15.0", "easysearch", b"NEW");
        let mut cfg = config(scratch.path(), "", &["easysearch"], "0.14.1");
        cfg.public_key_hex = Some(String::new());
        // An empty configured key must not silently fall back to the built-in
        // one: it is a malformed key and verification fails closed.
        assert!(matches!(
            Updater::new(cfg).check(&fetcher),
            Err(UpdateError::BadKey)
        ));
    }

    #[test]
    fn check_refuses_insecure_manifest_url() {
        let scratch = Scratch::new("insecure");
        let (fetcher, _sk, pk) = release_fixture(scratch.path(), "0.15.0", "easysearch", b"NEW");
        let mut cfg = config(scratch.path(), &pk, &["easysearch"], "0.14.1");
        cfg.manifest_url = "http://example.test/manifest.json".to_string();
        assert!(matches!(
            Updater::new(cfg).check(&fetcher),
            Err(UpdateError::Insecure(_))
        ));
    }

    #[test]
    fn install_replaces_binaries_atomically() {
        let scratch = Scratch::new("install");
        let payload = b"#!/bin/sh\necho new\n".to_vec();
        let (fetcher, _sk, pk) =
            release_fixture(scratch.path(), "0.15.0", "easysearch", &payload);
        let cfg = config(scratch.path(), &pk, &["easysearch"], "0.14.1");
        let up = Updater::new(cfg);

        // An older binary already exists.
        let bin = scratch.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let dest = bin.join("easysearch");
        std::fs::write(&dest, b"old").unwrap();

        let avail = up.check(&fetcher).unwrap().unwrap();
        let mut stages = Vec::new();
        let report = up
            .install(&avail, &fetcher, &mut |stage, _, _| stages.push(stage))
            .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        assert_eq!(report.files, vec![dest.clone()]);
        assert!(report.restart_required);
        assert!(stages.contains(&Stage::Downloading));
        assert!(stages.contains(&Stage::Verifying));
        assert!(stages.contains(&Stage::Installing));

        // The new file is executable.
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "expected the exec bits, got {mode:o}");

        // The staged copy is cleaned up.
        let leftovers: Vec<_> = std::fs::read_dir(scratch.path().join("cache"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".staged"))
            .collect();
        assert!(leftovers.is_empty(), "staged files should be removed");
    }

    #[test]
    fn install_rejects_a_tampered_asset_and_leaves_the_old_binary() {
        let scratch = Scratch::new("tamper");
        let (fetcher, _sk, pk) = release_fixture(scratch.path(), "0.15.0", "easysearch", b"GOOD");
        // The served bytes differ from the manifest's hash.
        let fetcher = fetcher.put("https://example.test/easysearch", b"EVIL".to_vec());
        let cfg = config(scratch.path(), &pk, &["easysearch"], "0.14.1");
        let up = Updater::new(cfg);

        let bin = scratch.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let dest = bin.join("easysearch");
        std::fs::write(&dest, b"old").unwrap();

        let avail = up.check(&fetcher).unwrap().unwrap();
        assert!(matches!(
            up.install(&avail, &fetcher, &mut |_, _, _| {}),
            Err(UpdateError::HashMismatch { .. })
        ));
        assert_eq!(std::fs::read(&dest).unwrap(), b"old");
    }

    #[test]
    fn install_refuses_when_no_asset_matches_this_install() {
        let scratch = Scratch::new("noasset");
        let (fetcher, _sk, pk) = release_fixture(scratch.path(), "0.15.0", "easysearch", b"NEW");
        // We only want a binary the release does not ship.
        let cfg = config(scratch.path(), &pk, &["easysearch-gui"], "0.14.1");
        assert!(matches!(
            Updater::new(cfg).check(&fetcher),
            Err(UpdateError::NoAsset(_))
        ));
    }

    #[test]
    fn missing_signature_is_a_fetch_error_not_a_pass() {
        let scratch = Scratch::new("nosig");
        let (fetcher, _sk, pk) = release_fixture(scratch.path(), "0.15.0", "easysearch", b"NEW");
        let mut files = fetcher.files;
        files.remove("https://example.test/manifest.json.sig");
        let fetcher = MemFetcher { files };

        let cfg = config(scratch.path(), &pk, &["easysearch"], "0.14.1");
        assert!(matches!(
            Updater::new(cfg).check(&fetcher),
            Err(UpdateError::Fetch(_))
        ));
    }

    /// The release tooling signs with `openssl pkeyutl`; prove that our verifier
    /// accepts those signatures (and the raw public key extraction the docs
    /// describe). Skipped when `openssl` is not installed.
    #[test]
    fn openssl_signatures_interoperate() {
        let run = |args: &[&str]| -> Option<std::process::Output> {
            let out = Command::new("openssl").args(args).output().ok()?;
            out.status.success().then_some(out)
        };
        if run(&["version"]).is_none() {
            eprintln!("skipping: openssl not available");
            return;
        }

        let scratch = Scratch::new("openssl");
        let key = scratch.path().join("key.pem");
        let msg = scratch.path().join("message.bin");
        let sig = scratch.path().join("message.sig");
        let message = b"{\"version\":\"0.15.0\"}";
        std::fs::write(&msg, message).unwrap();
        let (key_s, msg_s, sig_s) = (
            key.to_str().unwrap(),
            msg.to_str().unwrap(),
            sig.to_str().unwrap(),
        );

        assert!(run(&["genpkey", "-algorithm", "ED25519", "-out", key_s]).is_some());
        assert!(
            run(&[
                "pkeyutl", "-sign", "-rawin", "-inkey", key_s, "-in", msg_s, "-out", sig_s
            ])
            .is_some()
        );

        let der = run(&["pkey", "-in", key_s, "-pubout", "-outform", "DER"])
            .unwrap()
            .stdout;
        // SubjectPublicKeyInfo = a fixed 12-byte prefix + the raw 32-byte key.
        let pk_hex = hex_encode(&der[der.len() - 32..]);
        let sig_hex = hex_encode(&std::fs::read(&sig).unwrap());

        verify_signature(message, &sig_hex, &pk_hex)
            .expect("an openssl Ed25519 signature must verify");
    }
}
