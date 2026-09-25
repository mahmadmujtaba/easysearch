# Configuration

Everything for Linux reads one optional JSON file:

```
~/.config/everything-linux/config.json
```

(`$XDG_CONFIG_HOME/everything-linux/config.json` if `XDG_CONFIG_HOME` is set.)

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
  "persist_index": true,
  "storage": "sqlite",
  "db_dir": null,
  "disk_index_dir": null,
  "overlay_compaction_threshold": 8192,
  "exclude_fstypes": [],
  "content_index_enabled": false,
  "content_index_max_file_bytes": 8388608,
  "content_index_total_cap_bytes": 268435456,
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
| `respect_ignore_files` | `true` | Honour `.gitignore` / `.ignore` files inside the searched tree, plus the global ignore file at `~/.config/everything-linux/ignore`. This is what keeps `node_modules/`, `target/` and the like out of the index. |

## Where the index is stored

| Key | Default | Effect |
|---|---|---|
| `storage` | `"sqlite"` | `"sqlite"` keeps the index in a SQLite database (WAL mode) — the daemon is the only writer, and queries are answered from it. `"mmap"` uses the original memory-mapped binary index instead, which is faster for bare name queries but has no query language. See [`sqlite.md`](sqlite.md) for the trade-offs. |
| `db_dir` | `null` | Where the database lives. `null` means a `db/` folder beside the mmap index — so setting `disk_index_dir` moves the database too. |
| `persist_index` | `true` | `false` keeps **nothing** on disk: no database, no mmap file. Everything lives in RAM and is rebuilt on every start (useful for throwaway/portable use; a big `$HOME` costs real memory). |
| `disk_index_dir` | `null` | Where the mmap index lives (`index-v1.bin`). `null` means `$XDG_CACHE_HOME/everything-linux` (i.e. `~/.cache/everything-linux`). Only used when `storage` is `"mmap"`. |
| `overlay_compaction_threshold` | `8192` | mmap backend only: when the in-memory change overlay holds more than this many entries, a background compaction folds it into the file. The SQLite backend uses its own threshold (`REFRESH_AFTER_DIRTY`, 20 000). |

## Content search

Content search reads files **live** through the embedded ripgrep engine, so
results are always current. Separately, an optional in-RAM cache can speed up
repeated queries:

| Key | Default | Effect |
|---|---|---|
| `content_index_enabled` | `false` | `true` starts a background extractor that caches text from indexed files, making repeated content queries nearly free. Off means **zero** extra memory and zero background CPU. (Note: there is no UI switch for this yet — set it in this file and restart.) |
| `content_index_max_file_bytes` | `8388608` (8 MB) | Files larger than this are not cached (they are still searched live). |
| `content_index_total_cap_bytes` | `268435456` (256 MB) | Total size of the in-RAM cache; LRU-evicted beyond this. |

## Behaviour and limits

| Key | Default | Effect |
|---|---|---|
| `degraded_rescan_secs` | `30` | If the kernel watcher fails (typically exhausted inotify watch limits), fall back to a full rescan every N seconds. See `docs/scope.md` §5.2 for raising the watch limit instead. |
| `max_results` | `1000` | Cap on rows returned by a query. The GUI raises its own limit for the results table; this bounds the CLI/API. |

## Ignore file

`~/.config/everything-linux/ignore` — extra paths to exclude, in `.gitignore`
syntax (this is the "don't index these folders" file). The repository ships a
starting point:

```sh
cp .gitignore ~/.config/everything-linux/ignore
```

## Related files

| Path | Contents |
|---|---|
| `~/.config/everything-linux/config.json` | this file |
| `~/.config/everything-linux/gui.json` | GUI state: theme, zoom, open tabs, saved searches, search history (managed by the app) |
| `~/.config/everything-linux/ignore` | global ignore patterns |
| `~/.cache/everything-linux/db/index.db` | the SQLite index (+ `-wal`, `-shm`) |
| `~/.cache/everything-linux/index-v1.bin` | the mmap index, when `storage = "mmap"` |
| `~/.cache/everything-linux/daemon.log` | the daemon's log when started by the combined binary |
