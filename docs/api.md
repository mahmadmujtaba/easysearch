# Engine protocol

EasySearch runs the engine as a **child process** of the app. The app spawns it
(`easysearch --engine`, the same binary), keeps its stdin/stdout, and they
exchange JSON frames over those pipes. There is no socket, no listening port and
no HTTP: the pipes are private to the process pair, so nothing about the index is
reachable from anywhere else on the machine.

That is the whole point of the split — the index keeps running (and the window can
be closed and reopened) without a networked service to secure.

| Process | What it is |
| --- | --- |
| app (`easysearch`) | window + tray; spawns the engine, owns its pipes, kills it on exit |
| engine (`easysearch --engine`, or the standalone `easysearch-daemon`) | owns the index; answers frames on its stdin/stdout |
| `easysearch-gui` | the window alone (dev convenience): runs the engine in-process |
| `easysearch-cli` | the CLI: runs the engine in-process, so a script needs no app |

### Lifetime

The engine's lifetime is its parent's:

- The app spawns it at startup and **hides the window** (not the app) when the
  window is closed, so the engine keeps indexing; the tray's *Open* brings the
  window back. Quitting the app (tray *Quit*, *File ▸ Quit EasySearch*, or
  `easysearch --quit`) stops the engine too, and the loader reaps it.
- The engine's stderr goes to `$XDG_CACHE_HOME/easysearch/engine.log` (stdout is
  the protocol and is never used for logging — that is why the "index live" line
  goes to stderr).
- A crashed engine is noticed as a failed status poll; the app reports it in the
  status bar rather than dying with it.

> **One index owner:** exactly one engine should own the on-disk index at a time.
> The app is careful to spawn only one; if you also run the CLI, it opens the same
> index in-process, so prefer one or the other for heavy writing. Reads are safe
> (SQLite WAL).

## Frames

One JSON object per line, in both directions. A JSON string escapes its own
newlines, so a frame is always exactly one line and no length prefix is needed.

```text
→ {"id":1,"op":"search","payload":{…Query…}}
← {"id":1,"data":{…SearchResponse…}}
← {"id":2,"error":"bad regex: …"}
```

A reply carries **either** `data` **or** `error`, and repeats the request's `id`.
Requests are multiplexed by id: the engine answers each one on its own thread, so
a slow content search never blocks the status poller or the next query. Replies
are written whole, so they cannot interleave.

A request that cannot even be parsed is answered with `id: 0` and an `error`.

## Ops

| `op` | `payload` | `data` |
| --- | --- | --- |
| `health` | – | `{ok, version, api, uptime_secs}` |
| `status` | – | `{status:{…}, files, dirs}` |
| `search` | a `Query` object | `SearchResponse` |
| `count` | a `Query` object (its `limit` is ignored) | `{count:n}` |
| `rebuild` | – | – |
| `config` | `{respect?, follow_symlinks?, content_index?, rebuild?}` | – |
| `shutdown` | – | – |

`config` fields are all optional: only what is present changes. `respect` and
`follow_symlinks` are walk settings, so a change rebuilds the index (`rebuild`
defaults to `true`); `content_index` is the background content cache and takes
effect immediately. All three are reported in `status`
(`status.respect_ignore_files`, `status.follow_symlinks`, `status.content_index`).

### `Query`

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
  "fuzzy": false,
  "multiline": false,
  "content_or_name": false,
  "limit": 100
}
```

`name` is the Everything-style query (glob terms, `!term` to exclude) and
`content` an optional regex matched inside file contents; `content_or_name` makes
the two alternatives ("Full text") instead of both required. `category` accepts a
variant name (`"All"`, `"Images"`, …) or the parameterised form
(`{"Recent":{"max_age_secs":604800}}`). `under` restricts results to a directory
prefix (the GUI's *Locations* chips). `extensions` (canonicalised, deduped) and
`min_size` / `max_size` / `modified_within_secs` are optional; when set, folders
never match. `fuzzy` is fzf-style subsequence matching for the name terms;
`multiline` lets the content pattern span lines (slower).

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

Paths are sent as (lossy) UTF-8 strings; a path that is not valid UTF-8 is
replaced with U+FFFD.

## Driving it by hand

The standalone `easysearch-daemon` binary speaks the same protocol, so you can
type frames at it:

```sh
# One frame in, one frame out (stdout is protocol-only).
printf '{"id":1,"op":"health"}\n' | easysearch-daemon --quiet
```

`daemon/tests/roundtrip.rs` does exactly this to pin the wire format, and
`core/src/child.rs` is the client the app uses.
