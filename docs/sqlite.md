# SQLite index (work in progress)

The goal: the index lives in a **SQLite database** in a `db/` folder, the
**engine child keeps it up to date in realtime**, the GUI shows results
from it, the database is **created if missing**, and it is **refreshed** when a
lot of changes arrive at once.

## Status

**Done and active.** The engine now stores its index in SQLite by default; it
owns the database and keeps it current from kernel events, and every query
(GUI, CLI) is answered from it.

| Step | State |
| --- | --- |
| `rusqlite` (bundled SQLite) in `easysearch-core` | **Done** — builds here; no `pkg-config` needed |
| `core/src/sqlite_index.rs`: schema, create, rebuild, live deltas, `search`, `count`, `candidates`, `meta_of` | **Done** — 11 unit tests |
| Wired into `Engine` (base store, flush, refresh, counts) | **Done** |
| The live/realtime integration suite runs against it | **Done** — the whole `live_search` suite passes on SQLite |
| GUI access path | Through the engine child (decision 2 below) |

Verified end to end on a real `$HOME` (100,479 entries): the first run walks and
builds a 65 MB database; later runs serve from it immediately.

```
$ easysearch-cli search 'Cargo.toml'          # first run: builds the db
3 result(s), truncated in 18 ms (86100 files indexed)
$ easysearch-cli search '*.pdf' --min-size 10K
3 result(s), truncated in 2 ms
$ easysearch-cli search 'sqlite' --content 'min_size'    # ripgrep path
1 result(s) in 113 ms
```

## Files

* `core/src/sqlite_index.rs` — the backend.
* `core/src/config.rs` — `storage` (`sqlite` | `mmap`, default `sqlite`) and
  `db_dir` (default: a `db/` folder beside the mmap index, so redirecting
  `disk_index_dir` redirects the database with it).
* `core/src/engine.rs` — base store selection, `flush_overlay`, refresh policy,
  counts cache.

## Schema

```sql
files(
    id        INTEGER PRIMARY KEY,
    path      TEXT NOT NULL UNIQUE,
    name      TEXT NOT NULL,          -- basename
    dir       TEXT NOT NULL,          -- parent
    ext       TEXT NOT NULL,          -- lowercase, no dot, '' for directories
    size      INTEGER NOT NULL,
    mtime     INTEGER NOT NULL,       -- unix seconds
    is_dir    INTEGER NOT NULL,
    is_hidden INTEGER NOT NULL        -- same rule as matcher::is_hidden
)
meta(key TEXT PRIMARY KEY, value TEXT)   -- schema_version, built_at, …
```

Indexes on `name`, `dir`, `ext`, `size`, `mtime`. `schema_version` mismatch ⇒
the contents are dropped and the caller rebuilds.

## Design decisions

1. **One writer.** The engine owns the database and applies kernel events;
   readers only `SELECT`. WAL mode means readers never block the writer.
2. **Batched writes.** Events are committed in transactions
   (`SqliteIndex::apply`), never one `fsync` per event — per-event commits would
   be an order of magnitude slower than the current overlay.
3. **One predicate.** The coarse filters (directory prefix, extension, size,
   mtime, hidden, files-only) are pushed into SQL because they are *exactly* the
   conditions the engine applies. The name/category predicates then run in Rust
   through the same `accepts()` the mmap backend uses. The two backends cannot
   disagree about what matches — and the parity test asserts `count` ==
   `search().len()` for every query shape.
4. **Create vs refresh.**
   - *Create*: file missing, or `schema_version` differs, or the previous build
     never finished (the `complete` marker is missing — a partially filled
     database must never be mistaken for a small complete one).
   - *Refresh*: while running, `dirty()` counts applied changes; past
     `REFRESH_AFTER_DIRTY` (20 000) a full `rebuild` is cheaper and safer than
     replaying a very long delta stream. This is the "refresh when more changes
     are detected" behaviour, and the threshold is a single named constant.

## Trade-offs (worth knowing before wiring)

- **Bare-name queries will be slower than today.** An unfiltered query streams
  every row through SQLite and then through `accepts` in Rust; the mmap scan is
  a tight loop over a packed buffer. Filtered queries (ext/size/mtime/under) are
  narrow enough that SQLite should be competitive.
- **`LIKE '%foo%'` cannot use an index** in SQLite, so substring name search is
  a table scan. The current mmap design has the same asymptotics but a much
  smaller constant.
- **What SQLite buys**: durability and crash-safety, real SQL for the GUI's
  filters and facets, `FTS5` for content search (a genuine win over scanning),
  and no custom binary format to version.
- If SQLite ever loses badly on bare-name queries, the mitigation is to keep a
  narrow in-memory `name` index and only materialise rows for the winners.

## Wiring (as built)

1. `Config`: `storage` + `db_dir`.
2. `Engine::new`: open the database, seed the counts cache from it, and serve
   the existing index immediately (the mmap path's behaviour).
3. `Engine::start`: if the index is not *complete*, walk and `rebuild` on a
   background thread, going Live only when it finishes; otherwise re-validate in
   the background.
4. The watcher keeps writing to the in-memory overlay (unchanged);
   `flush_overlay()` folds a batch into the database in one transaction before
   every query, so a query never misses a change the watcher already saw.
5. `search` / `count` / `counts` / `meta_of` / `status_snapshot` delegate to the
   SQLite backend; `maybe_compact` flushes and applies the refresh policy.
6. The existing `live_search` integration suite now exercises all of this.

## Decisions taken

1. **Default backend: SQLite**, with `storage = "mmap"` in `config.json` as the
   escape hatch back to the memory-mapped file. RAM-only mode
   (`persist_index = false`) deliberately does not open a disk database.
2. **GUI access path: through the engine child.** The app talks to its engine
   child over pipes — the GUI and the engine are two processes of one app — so
   there is one writer and no lock contention.
3. **Refresh trigger: the delta backlog** — `REFRESH_AFTER_DIRTY` (20 000)
   applied changes since the last rebuild triggers a full rebuild on the
   background thread. No time-based rule, because idle time does not make an
   index stale: `apply()` keeps every change on disk within milliseconds of the
   kernel event.
