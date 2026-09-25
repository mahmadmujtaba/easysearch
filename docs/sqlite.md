# SQLite index (work in progress)

The goal: the index lives in a **SQLite database** in a `db/` folder, a
**background process keeps it up to date in realtime**, the GUI shows results
from it, the database is **created if missing**, and it is **refreshed** when a
lot of changes arrive at once.

## Status

| Step | State |
| --- | --- |
| `rusqlite` (bundled SQLite) added to `everything-core` | **Done** — compiles here via `gcc`; no `pkg-config` needed |
| `core/src/sqlite_index.rs`: schema, create, full rebuild, live deltas, `search`, `count` | **Done** — 7 unit tests, including a `search`/`count` parity check |
| Wiring it into `Engine` so the daemon owns it and the GUI/CLI read from it | **Not started** — see the plan below |
| GUI opening the database itself (read-only) | **Not started** — decision 3 below |

**The app still runs on the mmap index.** The module is in place, tested and
inactive; nothing you run today uses it yet.

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

1. **One writer.** The daemon owns the database and applies kernel events;
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
   - *Create*: file missing, empty, or `schema_version` differs ⇒ schema +
     full walk (`rebuild`).
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

## Wiring plan (next step)

1. `Config`: `storage: "sqlite" | "mmap"` (default `sqlite`), `db_dir`
   (default `$XDG_CACHE_HOME/everything-linux/db/`). Keep the mmap backend behind
   the config so the current fast path stays available.
2. `Engine::new`: open `SqliteIndex`; `needs_rebuild()` ⇒ full walk ⇒ `rebuild`;
   otherwise load counts and go live.
3. `build_entries()` (the walk) → `SqliteIndex::rebuild`.
4. Watcher: replace the overlay-then-compact cycle with a small in-memory batch
   that flushes to `SqliteIndex::apply` every ~200 ms, or immediately past N
   events. `dirty() > REFRESH_AFTER_DIRTY` ⇒ trigger `rebuild` on the background
   thread (the "refresh" path).
5. `Engine::search` / `count` / `counts` / `status` delegate to the SQLite
   backend when it is selected; `Status.base_entries` comes from `counts()`.
6. Tests: run the existing `live_search` integration suite against both
   backends and assert they return the same results.

## Open decisions

1. **Default backend.** SQLite as the default with `storage = "mmap"` as an
   escape hatch (my recommendation), or opt-in?
2. **GUI access path.** Read through the daemon (recommended — single writer, no
   lock contention, and the GUI already talks HTTP to it), or have the GUI open
   the database read-only itself as well (works if the daemon dies, adds a second
   connection and a second code path)?
3. **Refresh trigger.** Is "more than 20 000 changes since the last rebuild" the
   right rule, or do you want a time-based one (e.g. refresh if it has been more
   than X minutes and the tree changed), or both?
