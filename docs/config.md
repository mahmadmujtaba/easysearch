# Configuration

EasySearch reads one optional JSON file:

```
~/.config/easysearch/config.json
```

(`$XDG_CONFIG_HOME/easysearch/config.json` if `XDG_CONFIG_HOME` is set.)

The file is **optional**: if it is missing or unparseable, the defaults below are
used (an invalid file prints a warning and is ignored rather than crashing). All
keys are optional individually — a partial file keeps the default for anything
you leave out.

Restart the process after changing it. The GUI's own window state lives in a
separate file, `gui.json` (theme, zoom, tabs, saved searches, history), which the
app manages itself.

## Defaults

```json
{
  "roots": [],
  "exclude_removable": true,
  "exclude_network": true,
  "respect_ignore_files": true,
  "follow_symlinks": false,
  "persist_index": true,
  "storage": "sqlite",
  "db_dir": null,
  "disk_index_dir": null,
  "overlay_compaction_threshold": 8192,
  "exclude_fstypes": [],
  "content_index_enabled": false,
  "content_index_max_file_bytes": 8388608,
  "content_index_total_cap_bytes": 268435456,
  "content_index_in_memory": false,
  "degraded_rescan_secs": 30,
  "max_results": 1000
}
```

## What the index covers

| Key | Default | Effect |
|---|---|---|
| `roots` | `[]` | Directories to index. **Empty means `$HOME` of the user running the program** — the documented default scope. Add paths to index more, e.g. `["/home/me", "/srv/data"]`. Whole-filesystem indexing is an opt-in change, not a default. |
| `exclude_removable` | `true` | Skip removable media (USB sticks, card readers), detected via `/sys/block/*/removable`. |
| `exclude_network` | `true` | Skip network mounts (`nfs`, `cifs`/`smb`, `sshfs`, `gvfsd-fuse`, `9p`, …). |
| `exclude_fstypes` | `[]` | *Extra* filesystem types to skip, on top of the built-in list (pseudo filesystems, container overlays, network FS). Union, not replacement. |
| `respect_ignore_files` | `true` | Honour `.gitignore` / `.ignore` files inside the searched tree, plus the global ignore file at `~/.config/easysearch/ignore`. This is what keeps `node_modules/`, `target/` and the like out of the index. |
| `follow_symlinks` | `false` | Follow symbolic links into their targets while walking, so the contents of a symlinked folder are indexed too. Off by default (it can duplicate subtrees); cycles are detected and skipped. Both this and `respect_ignore_files` can be toggled live from **Settings ▸ Indexing** (the app pushes the change to the engine). |

## Where the index is stored

| Key | Default | Effect |
|---|---|---|
| `storage` | `"sqlite"` | `"sqlite"` keeps the index in a SQLite database (WAL mode) — the engine is the only writer, and queries are answered from it. `"mmap"` uses the original memory-mapped binary index instead, which is faster for bare name queries but has no query language. See [`sqlite.md`](sqlite.md) for the trade-offs. |
| `db_dir` | `null` | Where the database lives. `null` means a `db/` folder beside the mmap index — so setting `disk_index_dir` moves the database too. |
| `persist_index` | `true` | `false` keeps **nothing** on disk: no database, no mmap file. Everything lives in RAM and is rebuilt on every start (useful for throwaway/portable use; a big `$HOME` costs real memory). |
| `disk_index_dir` | `null` | Where the mmap index lives (`index-v1.bin`). `null` means `$XDG_CACHE_HOME/easysearch` (i.e. `~/.cache/easysearch`). Only used when `storage` is `"mmap"`. |
| `overlay_compaction_threshold` | `8192` | mmap backend only: when the in-memory change overlay holds more than this many entries, a background compaction folds it into the file. The SQLite backend uses its own threshold (`REFRESH_AFTER_DIRTY`, 20 000). |

## Content search

Content search reads files **live** through the embedded ripgrep engine, so
results are always current. Separately, an optional cache can speed up repeated
queries:

| Key | Default | Effect |
|---|---|---|
| `content_index_enabled` | `false` | Background content cache: extracted document text (`.docx`, OpenDocument `.odt`/`.ods`/`.odp`/`.odg`, PDF) is cached so repeated content searches do not re-extract it. **Off at boot** and **live-toggleable** — **Settings ▸ Indexing ▸ Background content index** (the app pushes it to the engine) — and switching it off frees the cache, so it costs nothing while off. |
| `content_index_max_file_bytes` | `8388608` (8 MB) | Files larger than this are not cached (they are still searched live). |
| `content_index_total_cap_bytes` | `268435456` (256 MB) | Total size of the cache; LRU-evicted beyond this. With the default disk store this bounds the spool directory; with `content_index_in_memory` it bounds resident memory. |
| `content_index_in_memory` | `false` | **Where** the cache lives: `false` spools text to `<disk_index_dir>/content/` and reads it back per lookup, so documents do not stay resident; `true` keeps it in RAM. Set at boot with `--content-in-memory` or `EASYSEARCH_CONTENT_MEMORY=1` (the launcher flag reaches the engine child the app spawns through the environment). |

## Behaviour and limits

| Key | Default | Effect |
|---|---|---|
| `degraded_rescan_secs` | `30` | If the kernel watcher fails (typically exhausted inotify watch limits), fall back to a full rescan every N seconds. See `docs/scope.md` §5.2 for raising the watch limit instead. |
| `max_results` | `1000` | Cap on rows returned by a query. The GUI raises its own limit for the results table; this bounds the CLI/API. |

## Ignore file

`~/.config/easysearch/ignore` — extra paths to exclude, in `.gitignore`
syntax (this is the "don't index these folders" file). The repository ships a
starting point:

```sh
cp .gitignore ~/.config/easysearch/ignore
```

There is a GUI for this (no hand-editing required): **Tools ▸ Ignore files…**
edits the file with a **Save & rebuild** button, and **Settings ▸ Indexing**
toggles `respect_ignore_files`. The toggle is **live**: it is pushed to the
running engine (the app's child process) and followed by a rebuild, so it takes
effect without restarting anything.

## Related files

| Path | Contents |
|---|---|
| `~/.config/easysearch/config.json` | this file |
| `~/.config/easysearch/gui.json` | GUI state: theme, zoom, open tabs, saved searches, search history (managed by the app) |
| `~/.config/easysearch/ignore` | global ignore patterns |
| `~/.cache/easysearch/db/index.db` | the SQLite index (+ `-wal`, `-shm`) |
| `~/.cache/easysearch/index-v1.bin` | the mmap index, when `storage = "mmap"` |
| `~/.cache/easysearch/engine.log` | the engine child's log |
