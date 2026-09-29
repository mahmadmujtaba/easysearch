# Using the GUI

EasySearch is one binary, `easysearch`, that runs as a **background host** (the tray and the
engine, `easysearch --daemon`) plus the **windows** it spawns as children (`easysearch --window`),
talking over the child's stdin/stdout as JSON frames ([`scope.md`](scope.md)) — no socket, port or
network code. `easysearch-cli` is the engine headless and `easysearch-gui` opens the window alone
(dev only); screenshots: [`dark`](screenshots/dark.png), [`light`](screenshots/light.png).

## Window layout

```
menu bar        File · Search · Filters · Tools · Settings · Help   [Free memory] [Reset defaults] [logo]
search tabs     one pill per open search (restored on restart)               [ + ]
toolbar         [ < │ > ]  Home  Index  [ Simple │ Regex │ Fuzzy ]  Recent  Saved   (left-aligned)
search row      [ query ]  [ scope ▾ ] [ location ▾ ]  [ Search ]
filter bar      Type Size Modified Path Ext: Case Hidden  [ ✕ Clear Filters ]
results header  N results · M files indexed · T ms   [ Filter results… ]   Sort by ▾  Cozy Compact
panes           sidebar | central view | preview/details;  below: view tabs · bulk actions · recent searches · status bar
```

**Menus:** File (tabs, focus, reload, export, find-by-hash, hide, quit) · Search (advanced search,
content, regex, fuzzy, multiline, case, hidden, full-path toggles) · Filters (preview pane, theme) · Tools (rebuild index, ignore files,
excluded folders, saved searches, restore from trash) · Settings (settings, zoom) · Help (about, shortcuts). The six
menu titles are colour-coded, and every button carries a tinted fill and a visible border that
lights up in the accent on hover.

**Toolbar:** a left-aligned strip of coloured-outline controls — only the border is tinted, the
fill stays the panel background, and hovering shows a description tooltip. **Back**/**Forward**
share one capsule; **Home** (a house) and **Index** (a database cylinder, which rebuilds the
index) are compact glyphs; **Simple** / **Regex** / **Fuzzy** are one mutually-exclusive
match-mode radio group; **Recent** (files changed in the last 7 days) and **Saved** (your saved
searches, with a count) close the row. Content search is no longer a toolbar button — pick the
*Contents (ripgrep)* or *Full text* scope in the search row instead.

**Quick actions** sit at the top right, just left of the logo, as two tinted
icon buttons: **Free memory** (a broom, teal) stops a running content search,
clears the current results — the largest thing the window holds — and asks the
engine to return freed pages to the operating system; **Reset defaults** (a
circular arrow, amber) puts every search setting, filter, tab and saved search
back to its default while keeping the theme and zoom you picked. Both act
immediately and report through a desktop notification that dismisses itself after
five seconds (via `notify-send`), not a dialog; the index, tags and files are
never touched. The window title carries the version and build stamp —
`EasySearch v0.50.0-20260929` — as does the logo's tooltip.

## Searching

Space-separated terms are **ANDed**; a plain term is a substring match (`draft` matches
`draft.pdf`). Queries are Everything-compatible:

| You type | Meaning |
|---|---|
| `!draft` | **exclude** matches (`!` stays literal even in Fuzzy mode) |
| `*.pdf` | glob: `*`, `?`, `[abc]` |
| `mtn` *(Fuzzy on)* | fzf-style subsequence: `mtn` → `meeting-notes.md` |
| `report[_-]\d{4}` | regex, while **Regex** is on |
| `foo\nbar` *(Multiline)* | content pattern spanning lines (much slower) |

**Filter tokens** can be typed in the query as well as set with the controls; a token applies for
that query and takes precedence over the matching filter control:

| You type | Meaning |
|---|---|
| `"a b"` | a phrase — one term that may contain spaces |
| `a|b` | either `a` or `b` (one term — binds tighter than the space/AND) |
| `ext:pdf,doc` | only these extensions (a leading `.` is optional) |
| `size:>10MB` | size bounds: `>`, `<`, `>=`, `<=`, or `a..b`; units `B`/`KB`/`MB`/`GB`/`TB` (binary) |
| `modified:today` | modified within `today` / `yesterday` / `week` / `month` / `year`, or `7d` / `12h` / `2w` |
| `in:/var/log` | only paths under this directory (`under:` works too) |
| `folder:docs` | only files whose immediate folder's *name* matches (`parent:` works too) |
| `file:` | files only |

An unrecognised `key:value` (like `time:12:30`) is matched literally.

**Search ▸ Advanced search…** builds a query from the same tokens with a small form — name
contains, extension, size (at least / at most), modified and folder — and puts the composed query
in the search box.

**Scope** (beside the search box): **Filenames** (default), **Full path**, **Contents
(ripgrep)** — inside files, read live so results are never stale — or **Full text** (name
**or** contents). It is **not** restored on restart; turning content search on shows a
one-time notice and a status-bar reminder. A content search only starts once the pattern
has **three characters** (a shorter one would scan everything for almost no signal — the
results header says so until then), and it is **case-insensitive** unless **Case** is on.

**Location** (and the sidebar's *Indexed Locations*) scopes to one directory; the toolbar's
Back/Forward walk your history. **Fuzzy** (toolbar or Settings) switches to fzf-style matching
and changes the ranking (*Relevance*, below).

## Filters

Every control maps to a real engine filter, applied the same way whether the app is counting
or listing:

| Control | Filter |
|---|---|
| **Type** | category: All / Recent / Images / Documents / Code / Archives / Audio / Video / Large files |
| **Size** | inclusive byte bounds (`< 1 MB`, `1–100 MB`, `100 MB–1 GB`, `> 1 GB`) |
| **Modified** | Today / Past week / Past month / Past year |
| **Path** | only paths inside one directory |
| **Ext:** | only these extensions (implies files-only) |
| **Case** / **Hidden** | case-sensitive matching; include dot-files |
| **Multiline** *(content)* | let the content pattern span lines; much slower |
| **✕ Clear Filters** | resets the controls (enabled only when something is set) |

## Results

Columns: `#` · **Name** (type badge; two-line with a breadcrumb in Cozy density; up to two tag
`#chips`) · **Path** · **Type** pill · **Size** · **Modified** · **Created** (birth time, read
live; `—` when the filesystem records none) · **Match** (query terms satisfied) · **Relevance**.
**Relevance** is a documented heuristic: name matches outrank path matches, exact beats prefix
beats substring, short names score higher, and a query with no positive terms scores everything
equally. **Sort by** offers Relevance (default), Name, Size, Modified or Created (header clicks
cycle ascending → descending → off); **Cozy**/**Compact** switch density; double-click opens.

Above the table, the **Filter results…** box narrows the visible rows by name or path
(case-insensitive) **without re-querying** the index — handy for pruning a large result set —
and the ✕ beside it clears the filter. **File ▸ Export results…** writes the visible rows to a
CSV, TSV or JSON file of your choosing (`name`, `path`, `size`, `modified`, plus `is_dir` in
JSON), and reports the destination through a desktop notification. **File ▸ Find files by hash…**
hashes the visible files (up to 2000, skipping folders and files over 512 MB) and lists those
matching a pasted SHA-256. Two checkboxes beside the filter box keep only **Empty** files and
folders or **Broken links** (broken symlinks). Select a row and press `F2` to rename it; the
name is checked for clashes before the rename, and the row updates in place.

Right-click a row for the context menu; it acts on the whole **selection** when the row is part
of one, otherwise on the row alone:

| Item | Notes |
|---|---|
| Open · Open containing folder · Open in terminal | |
| Copy path / **Copy N paths** · Copy name / **Copy N names** | one per line, sorted |
| **Copy as URI** · **Copy shell-escaped** | `file://…` links / quoted for a shell, one per line, sorted |
| Show in Details panel · Filter to this folder · Search for this name | switch pane / scope to folder / `*stem*` |
| **Tags…** · Filter by `#tag` · Clear tag filter | see *Tags* |
| Exclude folder · **Move to Trash…** · Add/Remove selection · Select all · Invert · Clear selection | folder rows / recoverable |
| **Find duplicates in results…** / **in selection (N)** | the latter needs 2+ checked |

Bulk **Select All**, **Invert** and **Copy paths** sit in the view-tab strip; a checked row is
shown as selected, so **Select All** visibly marks every row, and `Esc` clears the marks (a second
`Esc` clears the search).

## Sidebar

Its own **Search Everywhere** box filters the lists below. **Categories** mirror the *Type* filter,
each with a **live count** for the query; **Saved searches** store a named query with its filters
(`+` saves the current one; Tools ▸ *Saved searches…* manages them); **Tags** lists every tag with
its path count (click to filter, again to clear); **Indexed Locations** scopes to Home, Desktop,
Documents, Downloads, Pictures, Music, Videos, Workspace and Projects (whichever exist), each with
a count; **Advanced Search** holds *Include folders*, the **Excluded folders (N)…** link and the
content-index state. Hovering the **search bar** shows a small query cheat-sheet (`*.pdf` glob,
`!draft` exclude, `^src/` path prefix, `.*` regex).

## Preview and Details

The right-hand pane has **Preview** and **Details** tabs; ✕ hides it (re-open from Filters ▸
Preview pane) and ⧉ opens the file. **Preview** renders by kind: image thumbnails;
**audio/video** metadata (`ffprobe`) plus a video first frame (`ffmpeg`); the **text layer** of
PDF, `.docx` and OpenDocument files (`.odt`/`.ods`/`.odp`/`.odg`) extracted in-process;
**rendered Markdown**; and **syntax-highlighted source code** (~31 languages, and fenced code in
Markdown). Larger files fall back to plain monospace; `.xls`/`.xlsx`/`.ppt`/`.pptx` have no
preview. **Details:** Name · Path · Size (exact bytes) · Modified · Created · MIME type ·
Permissions · **Tags** · **SHA-256** (on demand, up to 512 MB). **Quick actions:** Open · Reveal ·
Copy path · Terminal · **Tag**.

## Status bar and tabs

The status bar shows live index state (`Indexing: idle (86,100 files)` or a spinner), a
content-search warning while active, **CPU**, **RAM**, a **UI zoom** dropdown (95 / 100 / 110 / 125 %,
on the right), keyboard hints, and the running **version** at the far right, plus `Entries:`,
`Search time:` and `Query: N results`. The **view tabs** at the bottom of the window —
**Results**, **Preview**, **Details** and **Search History** — switch what fills the central area
(the last lists your recent queries as clickable rows). Each search **tab** keeps its own query and
filters and is restored on the next start (the content scope is not); `Ctrl+T` opens one, `Ctrl+W`
closes it, `Ctrl+Tab` cycles and `Ctrl+1…9` jumps. The **Recent searches** row shows recent queries
as chips (click to re-run) and mirrors them in the tray menu; a *saved search* bundles a query with
its filters, and tabs and history live in `~/.config/easysearch/gui.json`.

## Finding duplicates

Right-click ▸ **Find duplicates in results…** (or **in selection** with rows checked) scans for
files with *identical contents*. Only files count, empty files are ignored, and hardlinks to
one inode are collapsed to a single entry (the same file is not its own duplicate); candidates
are grouped by size and compared by size + the first 64 KiB before being hashed in full
(SHA-256);
groups of more than one are shown largest waste first, capped at 50 000 candidates (`scan
capped` past that).

Tick copies to remove — per copy, a group's **Keep newest** / **Select all**, or the bulk
**Select all but one per group** / **Clear selection** — then **Move N to Trash…**. Files go to
the freedesktop **Trash** (`~/.local/share/Trash`) with their original path recorded, so any
file manager can restore them; the confirmation lists what will move, and a whole group being
checked warns first. **Move to Trash…** is also on a row's menu.

**Tools ▸ Restore from Trash…** (also in the command palette) lists what you trashed — from
this app or your file manager — newest first, with each item's original folder. **Restore** puts
one file back and **Restore all** puts every item back; a folder that was removed is recreated,
and a name that is already taken gets a numeric suffix rather than being overwritten.

## Tags

Tags are your own labels on files and folders — “work”, “2026”, “archive” — stored locally in
`~/.config/easysearch/tags.json`; they are a view over the results, not part of the index (the
engine never sees them, and nothing is written to the tagged files).

- **Edit** them from a row's **Tags…** menu item; opened from a selected row, the editor acts on
  the whole checked selection, so one action can tag many files (a tag on only some of the
  selection reads `#tag (2/5)`). The **Tag** quick action in the preview opens it for the file
  you are viewing.
- **Filter** by clicking a `#chip` (in a result row, the preview, or the results header) or a
  tag in the sidebar's **TAGS** section; the active filter shows in the results header with a ✕
  to clear it. Tags follow **paths**, so renaming or moving a file outside EasySearch leaves the
  tag on the old path.

## Ignoring files, symlinks and excluded folders

Indexing honours `.gitignore` / `.ignore` files in the tree plus a global list at
`~/.config/easysearch/ignore`. **Tools ▸ Ignore files…** edits that list (`.gitignore` syntax;
`**/` matches at any depth) with **Save & rebuild**, **Reload** and **Rebuild index**, plus two
live switches: **Honor `.gitignore` / `.ignore` files** and **Follow symbolic links** (off by
default; cycles are skipped), both pushed to the running engine and followed by a rebuild.
**Settings ▸ Indexing** has the same switches and **Background content index** — an optional
cache of text extracted from Office/PDF files, off at boot and spooled to disk by default
(`--content-in-memory` keeps it in RAM).

**Excluded folders** are a *path* exclusion, separate from ignore patterns. **Tools ▸ Excluded
folders…** edits `config.exclude_dirs` — exact directory trees to skip, one path per line (`~` =
home) — and saves `config.json` with a live rebuild; right-clicking a folder result ▸ **Exclude
folder from the index** adds one on the spot, and the sidebar's **Excluded folders (N)…** row
shows the count and opens the dialog ([`config.md`](config.md)).

## Theming and fonts

Settings ▸ Appearance and **Filters ▸ Theme** offer **Dark** (default), **Light** and **Brand**,
a deep-teal palette drawn from the logo, remembered by name in `gui.json`. Fonts come from the
system: the desktop's configured family is resolved with `fc-match` (KDE, then GTK, then
fontconfig) for the UI and the monospace face, with the bundled egui faces kept only as glyph
fallbacks. Sidebar and result icons are painted in the theme's colours.

## Tray, closing and the global hotkey

The **X** closes the window and leaves EasySearch running: the window process
exits, while the background host keeps the tray icon and the engine indexing, so
the search stays warm and the next open is instant. Only **Quit** stops the app
and its engine: *File ▸ Quit EasySearch*, the tray's *Quit EasySearch*, or
`easysearch --quit`.

**Settings ▸ Startup ▸ *Start EasySearch at login (background)*** writes an XDG
autostart entry (`~/.config/autostart/easysearch.desktop`) that runs
`easysearch --hidden`, so the tray and the index are ready at login without
opening a window. Unchecking it removes the entry. Launching from the desktop
(`easysearch` with no arguments) still opens a window.

**Left-click** (or middle-click) the tray icon to **flip** the window: it opens
ready to search if it is hidden, and hides again if it is visible. **Right-click**
opens the menu:

| Tray item | Effect |
|---|---|
| *Open EasySearch* | show the window and focus the search box |
| *Show / Hide window* | flip the window on and off |
| *New search* | show, clear the current results and focus the search box |
| *Clear results* | empty the open window's result list (frees the rows) |
| *Recent searches ▸* | re-run a recent query; *Clear history* empties the list |
| *Rebuild index* | rebuild the on-disk index |
| *Open index folder* | reveal the index folder in the file manager |
| *Settings…* / *About* | open the matching dialog in the window |
| *Quit EasySearch* | stop the app and its engine |

Reopening from the tray or `easysearch --show` starts a fresh window process
against the same, still-warm index.

Why it works this way: on **Wayland** a window cannot be unmapped and `winit`
cannot recreate its event loop, so a closed window could not be reopened inside
one process. The host/window split sidesteps both — the window really exits and a
new one is spawned on demand. See [`scope.md`](scope.md).

Wayland has no global-hotkey API, so the app ships a control socket
(`$XDG_RUNTIME_DIR/easysearch.sock`, mode `0600`) driven from the command line — bind one of
these as a desktop shortcut:

| Command | Effect |
|---|---|
| `easysearch --toggle` / `--show` / `--hide` | toggle / show / hide the window |
| `easysearch --search TODO` | show the window and run a search |
| `easysearch --quit` | stop the app and the engine |

The first `--toggle`, `--show` or `--search` **starts** the app when nothing is running; `--hide` and `--quit` need a running instance.

## Keyboard shortcuts

| Key | Action |
|---|---|
| `↑` `↓` `PgUp` `PgDn` | Navigate results |
| `Enter` / double-click | Open the selected result |
| `Ctrl+Enter` | Open the containing folder |
| `F5` | Re-run the search |
| `Alt+↑` | Go to the parent location |
| `F2` | Rename the selected file |
| `Esc` | Clear the row selection, then the search |
| `Ctrl+F` | Focus the search box |
| `Ctrl+A` | Select all result rows (marks them) |
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+T` / `Ctrl+W` | New tab / close tab |
| `Ctrl+Tab` / `Ctrl+1…9` | Next tab / select tab |
| `↑` `↓` in an empty search box | Cycle search history |
| Click column headers | Sort results |

**Focus follows the pointer:** a press outside the search field — or moving the pointer into the results — drops its focus, so
`↑`/`↓`/`PgUp`/`PgDn`/`Enter` and `Ctrl+A` act on the results, and the highlighted row tracks the cursor as it moves over
the list. The first printable key re-grabs the field to start a new query.
