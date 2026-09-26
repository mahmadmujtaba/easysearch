# Configuration

EasySearch reads one optional JSON file — `~/.config/easysearch/config.json`
(`$XDG_CONFIG_HOME/easysearch/config.json` when `XDG_CONFIG_HOME` is set).

The file is **optional**: if it is missing or unparseable the defaults below are
used (an invalid file warns on stderr and is ignored). Every key is optional on
its own, so a partial file keeps the default for whatever it leaves out; restart
after editing. The app manages its own `gui.json` (theme, zoom, tabs, saved
searches, history) and `tags.json` (per-path tags) alongside it.

## Defaults

```json
{
  "roots": [], "exclude_removable": true, "exclude_network": true, "exclude_dirs": [],
  "respect_ignore_files": true, "follow_symlinks": false, "persist_index": true,
  "storage": "sqlite", "db_dir": null, "disk_index_dir": null, "overlay_compaction_threshold": 8192,
  "exclude_fstypes": [], "content_index_enabled": false, "content_index_max_file_bytes": 8388608,
  "content_index_total_cap_bytes": 268435456, "content_index_in_memory": false,
  "degraded_rescan_secs": 30, "max_results": 1000
}
```

## What the index covers

| Key | Default | Effect |
|---|---|---|
| `roots` | `[]` | Directories to index. Empty means `$HOME` of the user running the program — the documented default scope. Whole-filesystem indexing is opt-in. |
| `exclude_removable` | `true` | Skip removable media (USB sticks, card readers), via `/sys/block/*/removable`. |
| `exclude_network` | `true` | Skip network mounts (`nfs`, `cifs`/`smb`, `sshfs`, `gvfsd-fuse`, `9p`, …). |
| `exclude_dirs` | `[]` | Exact directory trees to leave out, one path per entry. `~` expands to `$HOME`; a relative path is taken from `$HOME`. Live-editable in **Tools ▸ Excluded folders…**, which saves this key and rebuilds. |
| `exclude_fstypes` | `[]` | *Extra* filesystem types to skip, on top of the built-in list (pseudo filesystems, overlays, network FS). Union, not replacement. |
| `respect_ignore_files` | `true` | Honour `.gitignore` / `.ignore` files in the tree plus the global `~/.config/easysearch/ignore` file. Keeps `node_modules/`, `target/` and the like out of the index. |
| `follow_symlinks` | `false` | Follow symlinks into their targets while walking, so symlinked folders are indexed too. Off by default (can duplicate subtrees); cycles are skipped. |

`respect_ignore_files` and `follow_symlinks` are also toggled live from
**Settings ▸ Indexing**, which pushes the change to the engine.

## Where the index is stored

| Key | Default | Effect |
|---|---|---|
| `storage` | `"sqlite"` | `"sqlite"` keeps the index in a SQLite database (WAL mode); the engine is the only writer. `"mmap"` uses the memory-mapped binary index instead — faster name queries, no query language. See [`scope.md`](scope.md). |
| `db_dir` | `null` | Where the database lives. `null` means a `db/` folder beside the mmap index, so setting `disk_index_dir` moves the database too. |
| `persist_index` | `true` | `false` keeps **nothing** on disk; everything is rebuilt in RAM each start (portable/throwaway use, but a large `$HOME` costs real memory). |
| `disk_index_dir` | `null` | Where the mmap index (`index-v1.bin`) lives. `null` means `$XDG_CACHE_HOME/easysearch` (`~/.cache/easysearch`). Only used when `storage = "mmap"`. |
| `overlay_compaction_threshold` | `8192` | mmap only: when the in-memory change overlay exceeds this, a background compaction folds it into the file. The SQLite backend uses its own threshold (20 000). |

## Content search

Content search reads files **live** through the embedded ripgrep engine, so
results are always current. An optional cache can speed up repeated queries:

| Key | Default | Effect |
|---|---|---|
| `content_index_enabled` | `false` | Background cache of extracted document text (`.docx`, OpenDocument `.odt`/`.ods`/`.odp`/`.odg`, PDF), so repeated content searches do not re-extract. Off at boot and live-toggleable (**Settings ▸ Indexing ▸ Background content index**); switching it off frees the cache. |
| `content_index_max_file_bytes` | `8388608` (8 MB) | Files larger than this are not cached (they are still searched live). |
| `content_index_total_cap_bytes` | `268435456` (256 MB) | Cache size cap; LRU-evicted beyond this. Bounds the spool directory, or resident memory with `content_index_in_memory`. |
| `content_index_in_memory` | `false` | `false` spools text to `<disk_index_dir>/content/` and reads it back per lookup; `true` keeps it in RAM. Set at boot with `--content-in-memory` or `EASYSEARCH_CONTENT_MEMORY=1`. |

## Behaviour and limits

| Key | Default | Effect |
|---|---|---|
| `degraded_rescan_secs` | `30` | If the kernel watcher fails (typically exhausted inotify watch limits), fall back to a full rescan every N seconds. See [`scope.md`](scope.md) (realtime design). |
| `max_results` | `1000` | Cap on rows returned per query. Bounds the CLI/API; the GUI raises its own limit for the results table. |

## Ignore file

`~/.config/easysearch/ignore` holds extra paths to exclude, in `.gitignore`
syntax. The repository ships a starting point:

```sh
cp .gitignore ~/.config/easysearch/ignore
```

**Tools ▸ Ignore files…** edits it with a **Save & rebuild** button, and
**Settings ▸ Indexing** toggles `respect_ignore_files` live — the change is
pushed to the engine and followed by a rebuild.

## Related files

| Path | Contents |
|---|---|
| `~/.config/easysearch/config.json` | this file |
| `~/.config/easysearch/gui.json` | GUI state: theme, zoom, open tabs, saved searches, history |
| `~/.config/easysearch/tags.json` | per-path tags added in the GUI |
| `~/.config/easysearch/ignore` | global ignore patterns |
| `~/.cache/easysearch/db/index.db` | the SQLite index (+ `-wal`, `-shm`) |
| `~/.cache/easysearch/index-v1.bin` | the mmap index, when `storage = "mmap"` |
| `~/.cache/easysearch/engine.log` | the background host's log |
