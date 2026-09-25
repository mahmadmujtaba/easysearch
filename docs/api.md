# Daemon HTTP API

`everything-daemon` runs the search engine as its own process and serves it over
a small HTTP/JSON API on **localhost only** (`127.0.0.1:5858` by default).

Splitting the engine out of the GUI means the index keeps running — and any
other client keeps working — even when no GUI is running. Clients:

| Client | How it talks to the engine |
| --- | --- |
| `everything-linux` | **the single-file app**: opens the GUI and starts a daemon by re-executing itself when none is listening (falling back to an in-process engine if it cannot). This is what end users run. |
| `everything-gui` | the GUI alone (dev convenience): attaches to a daemon (`--daemon ADDR`, `EVERYTHING_DAEMON`, or auto-detected on the default address) |
| `everything` (CLI) | in-process by default; `--remote ADDR` queries a daemon |
| anything else | plain HTTP + JSON (`curl`, scripts, another language) |

### Sharing the single binary

```sh
make dist          # → dist/everything-linux  (one self-contained file)
```

The GUI and the daemon are the same executable: running it starts a daemon (the
same file, re-executed with `--daemon`, detached into its own session) and
attaches the GUI to it. The daemon's log goes to
`$XDG_CACHE_HOME/everything-linux/daemon.log`. It needs only the usual desktop
libraries (OpenGL/EGL and the windowing stack) that any graphical Linux
installation already has.

> **Security:** the API has no authentication and is bound to loopback. Do not
> bind it to a non-loopback address or expose it through a proxy.
>
> **One index owner:** only one process should own the on-disk index at a time.
> When a daemon is running, have the GUI/CLI attach to it rather than starting a
> second in-process engine — otherwise both rewrite the same index cache (the
> writes are atomic renames, so nothing is corrupted, but the work is duplicated).

## Running

```sh
make daemon                              # release build, 127.0.0.1:5858
./target/release/everything-daemon --addr 127.0.0.1:5858 --quiet
```

## Endpoints

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/v1/health` | `{ok, version, api, uptime_secs}` — liveness/version probe |
| `GET` | `/v1/status` | `{status:{…}, files, dirs}` — index/watcher state |
| `POST` | `/v1/search` | body: a `Query` object → `SearchResponse` |
| `GET` | `/v1/search` | same, with query parameters (see below) |
| `POST` | `/v1/count` | body: a `Query` object → `{"count":n}` (counts only, no rows) |
| `POST` | `/v1/rebuild` | rebuild the on-disk index in the background → `{ok:true}` |
| `POST` | `/v1/ignore` | body `{"respect":bool,"rebuild":bool}` — honor `.gitignore`/`.ignore` files (and the global ignore file) or not → `{ok:true}`. The new value is reported as `status.respect_ignore_files` in `/v1/status`. |
| `POST` | `/v1/shutdown` | ask the daemon to stop and exit → `{ok:true}` (used to restart onto a freshly installed binary) |
| `GET` | `/v1/watch` | long-poll: the `/v1/status` payload, returned when it changes or after `?timeout=<secs>` (default 25, max 120) |

Errors are always JSON: `{"error":"…"}` with a `4xx`/`5xx` status.

### `GET /v1/search` parameters

| Parameter | Meaning |
| --- | --- |
| `query` / `q` | Everything-style name query (glob terms, `!` to exclude) |
| `regex` | `1`/`true` — treat terms as regex |
| `case` | `1`/`true` — case-sensitive |
| `content` | regex searched inside file contents |
| `hidden` | `1`/`true` — include hidden files |
| `path` | `1`/`true` — match the full path, not just the basename |
| `dirs` | `0`/`false` — exclude folders from results |
| `under` | only return paths inside this directory (e.g. `under=/home/me/Downloads`) |
| `ext` | comma-separated file extensions (e.g. `ext=pdf,docx,md`, case-insensitive, leading `.` optional); when set, directories are excluded and only these final extensions match |
| `min_size` | minimum file size in bytes, inclusive; when set, directories are excluded |
| `max_size` | maximum file size in bytes, inclusive; when set, directories are excluded |
| `modified_within` | only entries modified within the last N seconds (alias `modified_within_secs`; future mtimes always match) |
| `limit` | maximum results (default 1000) |
| `category` | `all`, `recent`, `images`, `docs`, `code`, `archives`, `audio`, `video`, `large` |

Unknown parameters are a `400` (fail loudly rather than silently ignoring).

### `Query` object (`POST /v1/search`)

```json
{
  "name": "invoice 2026 *.pdf",
  "regex_mode": false,
  "case_sensitive": false,
  "include_hidden": false,
  "full_path": false,
  "content": null,
  "category": "All",
  "include_dirs": true,
  "under": null,
  "extensions": [],
  "min_size": null,
  "max_size": null,
  "modified_within_secs": null,
  "limit": 100
}
```

`category` accepts either a variant name (`"All"`, `"Images"`, …) or the
parameterised form used internally, e.g. `{"Recent":{"max_age_secs":604800}}`.
`under` is an optional directory prefix — only paths inside it are returned —
and is what the GUI uses for its sidebar *Locations* chips.

`extensions` (default `[]`) restricts files to these final extensions; entries
are canonicalised (whitespace trimmed, one leading `.` stripped, lowercased,
deduped) and, when the list is non-empty, directories never match.
`min_size` / `max_size` (default `null`) are inclusive byte bounds; when either
is set, directories never match. `modified_within_secs` (default `null`) keeps
entries whose age `now - mtime` is at most that many seconds. All three fields
are optional, so older clients that omit them keep working.

### `SearchResponse`

```json
{
  "results": [
    {"path": "/home/me/report.pdf", "size": 51234, "mtime": 1758700000, "is_dir": false}
  ],
  "truncated": false,
  "elapsed_ms": 3,
  "indexed": 198550
}
```

### `CountDto` (`POST /v1/count`)

```json
{ "count": 4213 }
```

`/v1/count` takes the same `Query` object as `/v1/search` (its `limit` is
ignored) and returns only how many entries match — the engine walks the index
with the same predicate as a search, so the two always agree. The GUI uses this
to fill the per-category counts in its sidebar without transferring rows.

Paths are sent as (lossy) UTF-8 strings; a path that is not valid UTF-8 is
replaced with U+FFFD.

## Examples

```sh
# Liveness
curl -s http://127.0.0.1:5858/v1/health

# All PDFs, newest-first is up to the client — the API returns index order
curl -s 'http://127.0.0.1:5858/v1/search?query=*.pdf&limit=20'

# Regex content search
curl -s -X POST http://127.0.0.1:5858/v1/search \
  -H 'Content-Type: application/json' \
  -d '{"name":"","content":"fn main\\(","regex_mode":true,"limit":10,"category":"All","include_dirs":false,"case_sensitive":false,"include_hidden":false,"full_path":false}'

# How many results would there be? (no rows transferred)
curl -s -X POST http://127.0.0.1:5858/v1/count \
  -H 'Content-Type: application/json' \
  -d '{"name":"*.pdf","category":"All","include_dirs":false,"limit":1}'

# Everything under one directory only
curl -s 'http://127.0.0.1:5858/v1/search?query=*.log&under=/home/me/Downloads&limit=20'

# Smallest files only, via jq
curl -s 'http://127.0.0.1:5858/v1/search?query=*.log&limit=50' | jq -r '.results[] | "\(.size)\t\(.path)"' | sort -n

# Follow index changes (long-poll loop)
while true; do curl -s 'http://127.0.0.1:5858/v1/watch?timeout=25' | jq -c '.status.state'; done

# CLI against the daemon
everything --remote 127.0.0.1:5858 search 'invoice 2026' --limit 20
everything --remote 127.0.0.1:5858 status
```

## Why long-polling instead of SSE/WebSocket

Change notification uses long-polling (`/v1/watch`). It works with every HTTP
client, needs no framing layer, and is not defeated by the server's chunk
encoder: `tiny_http` streams through `chunked_transfer::Encoder`, which buffers
small writes, so short server-sent-event frames would never be flushed to the
client. A WebSocket endpoint would require an additional framing + handshake
implementation for little gain over long-polling at this scale.
