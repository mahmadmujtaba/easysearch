# Using the GUI

The window follows the “FileSearch Pro” reference layout
(`ui-screenshots/main1.png`). This guide explains every part of it and what is
real underneath.

## Window layout

```
menu bar        File · Search · Filters · Tools · Settings · Help
search tabs     one pill per open search (restored on restart)      [ + ]
toolbar         Back Forward Home Index | Content Search Regex Fuzzy | Recent Saved
search row      Search: [ query ]  [ scope ▾ ] [ location ▾ ]  [ ⌕ Search ]
filter bar      Type Size Modified Path Ext: Case Hidden   [ ✕ Clear Filters ]
results header  31 results · 100,477 files indexed · 2 ms   Sort by ▾  Cozy Compact
┌───────────────┬───────────────────────────────────────────┬──────────────────┐
│ sidebar       │ results table                             │ preview panel    │
│ categories    │ #  Name  Path  Type  Size  Modified  Created  Match  │ Preview │ Details│
│ saved search  │                                             │ details + actions│
│ locations     │                                             │                  │
│ advanced      │                                             │                  │
└───────────────┴───────────────────────────────────────────┴──────────────────┘
view tabs       Results · Preview · Details · Search History | bulk actions
recent row      Recent searches: chips…                          Clear
status bar      Indexing: idle (86,100 files) CPU RAM Zoom | Query Search time Indexed · hints · version
```

## Searching

Query syntax (Everything-compatible):

| You type | Meaning |
|---|---|
| `report` | substring match on the file name (`draft` matches `draft.pdf`) |
| `mtn` *(with Fuzzy on)* | subsequence match — `mtn` finds `meeting-notes.md` |
| `*.pdf` | glob (`*`, `?`, `[abc]`) |
| `report 2026` | several terms are **ANDed** |
| `!draft` | **excludes** matches |
| `^src/` | path prefix (with the *Full path* scope) |
| `report[_-]\d{4}` | a regex, when `Regex` is on |
| `foo\nbar` *(Multiline)* | a content regex that spans lines (much slower) |

**Scope** (the picker next to the search box) decides *what* the query is matched
against: **Filenames** (default), **Full path**, **Contents** (inside files, via
the embedded ripgrep engine — always read live, never stale), or **Full text**,
where the query matches the file name **or** its contents. *Contents* and *Full
text* use the same pattern for content matching; the difference is whether the
name must also match (`Contents`) or may match instead (`Full text`).

The scope is **not** restored on restart: a fresh start is always **Filenames**,
because content search is the expensive one and should be a deliberate choice.
Switching to *Contents* / *Full text* shows a one-time notice (content search
reads your files whole and so uses noticeably more memory and CPU than filename
search) and adds a reminder to the status bar while it is on. Switching **back**
to *Filenames* drops the rows the content search produced and asks the engine to
release its pages, rather than leaving them resident until the next search.

**Location** (and the sidebar's *Indexed Locations*) restricts the search to one
directory; Back/Forward in the toolbar walk your location history.

**Fuzzy** (the toolbar button, `Search ▸ Fuzzy matching`, or `Settings ▸ Search`)
switches matching to fzf-style subsequences: the query's characters must appear
*in order* anywhere in the name, so `mtn` finds `meeting-notes.md`. It is a global
mode rather than a per-tab one. Fuzzy mode also changes the ranking — the
**Relevance** column and sort use a score that rewards word-start and contiguous
matches and penalises gaps — so the best match rises to the top. Exclusions
(`!term`) are not made fuzzy: they keep their literal substring meaning.

## Filters

Every control in the filter bar maps to a real engine filter, applied by the same
predicate whether the app is counting or listing:

| Control | Underlying filter |
|---|---|
| **Type** | `category`: All / Recent / Images / Documents / Code / Archives / Audio / Video / Large files |
| **Size** | inclusive byte bounds (`< 1 MB`, `1–100 MB`, `100 MB–1 GB`, `> 1 GB`) |
| **Modified** | modified within Today / Past week / Past month / Past year |
| **Path** | `under` — only paths inside a directory |
| **Ext:** | only these extensions (implies files-only) |
| **Case** / **Hidden** | case-sensitive matching; include dot-files |
| **Multiline** *(content mode)* | let the content pattern span lines (`foo\nbar`); much slower |
| **✕ Clear Filters** | resets all of the above (enabled only when something is set) |

## Ignoring files and following symlinks

Indexing honours `.gitignore` and `.ignore` files found in the tree, plus a
global list at `~/.config/easysearch/ignore`. Both are managed without
hand-editing:

- **Tools ▸ Ignore files…** opens an editor for the global list (`.gitignore`
syntax; `**/node_modules/` matches at any depth) with **Save & rebuild**,
**Reload** and **Rebuild index**, plus two live switches:
  - **Honor `.gitignore` / `.ignore` files** — off means index everything.
  - **Follow symbolic links** — index the targets of symlinked folders too
    (off by default; cycles are detected and skipped).
- **Settings ▸ Indexing** offers the same two switches and a link to the editor,
  plus **Background content index** (see below).

Both switches are **live**: they are pushed to the running engine and followed by
a rebuild, so they take effect immediately rather than at the next restart.

**Excluded folders** are a *path* exclusion, separate from the ignore patterns:
**Tools ▸ Excluded folders…** edits a list of exact directory trees to leave out
of the index (one absolute path per line; `~` means your home folder), and
right-clicking a folder result ▸ **Exclude folder from the index** adds one on the
spot. Saving writes `exclude_dirs` in `config.json` and rebuilds without those
subtrees; the sidebar's **Excluded folders (N)…** row in *Advanced Search* opens
the same dialog and shows the count.

**Background content index** (`Settings ▸ Indexing`) caches the text it extracts
from `.docx`/`.odt`/`.pdf` files so repeated content searches are nearly free.
It is off by default, and it only ever holds *extracted document text* (plain
text files are read live). It is live too: turning it on starts caching lazily as
you search, and turning it off drops what is cached, so it costs nothing while
off — the sidebar's `cache …` indicator in the status bar shows its state.

## Results

Columns: `#` row number · **Name** (with a type badge and, in Cozy density, a
clickable breadcrumb) · **Path** (parent, shortened — hover for the full path) ·
**Type** pill · **Size** · **Modified** · **Created** (birth time) · **Match**
(which query terms this hit satisfied) · **Relevance** (score + bar).

**Created** is read live with `stat` for the visible rows rather than stored in
the index, so it is right even for a file made a second ago and needs no format
change. Where the filesystem records no birth time (many do not) the cell shows
`—`, and those rows sort together at one end.

**Relevance is a documented heuristic, not a black box.** Terms that match the
file *name* score highest — exact match, then prefix, then substring — with
matches anywhere in the path scoring less; a small bonus goes to short names
because they are usually the more specific hit; a query with no positive terms
scores everything equally. The best possible score is 100%. Use `Sort by →
Relevance` (the default) to order by it.

Header clicks sort by Name / Size / Modified / Created; clicking again reverses,
a third time clears the sort. **Cozy / Compact** switch the row density (two-line rows
with breadcrumbs, or one compact line).

Right-click a row for the full menu — it acts on the whole **selection** when the
row is part of one, otherwise on the row alone:

| Item | Notes |
|---|---|
| Open · Open containing folder · Open in terminal | |
| Copy path / **Copy N paths** · Copy name / **Copy N names** | one per line, sorted |
| Show in Details panel | selects the row and switches the panel |
| Filter to this folder | sets the location filter to the row's directory |
| Search for this name | new query for `*stem*` |
| Add to / Remove from selection | |
| Select all results · Invert selection · Clear selection | |
| **Find duplicates in results…** | scans the whole result set |
| **Find duplicates in selection (N)** | scans only the checked rows (needs 2+) |

Double-click (or Enter) opens with the default app.

## Sidebar

- **Search Everywhere** — filters the sidebar lists themselves.
- **Categories** — All files / Recent / Images / Documents / Code / Archives /
  Audio / Video / Large files, each with a **live count** for the current query.
- **Saved searches** — name a query + filters and reuse it later (`+` saves the
  current one; the toolbar's *Saved* button and the Tools menu open the manager,
  where you can Apply or Delete).
- **Indexed locations** — one-click scoping to Home, Desktop, Documents,
  Downloads, Pictures, Music, Videos, Workspace, Projects (whichever exist),
  each with its own **count** for the current query — so you can see where the
  matches actually are before narrowing to one.
- **Advanced search** — *Include folders* (folders in results). The switches
  that change what the index contains (ignore files, symlinks, the background
  content index) live in **Settings ▸ Indexing** and **Tools ▸ Ignore files…**.
- **TIPS** — a collapsible cheat-sheet; its state is remembered.

## Search tabs and history

Each tab keeps its own query and filters, and everything is restored the next
time you start the app. The **Recent searches** row shows recent queries as
chips (click to re-run); they are also mirrored in the tray menu, and Ctrl+A
selects every result row for bulk actions (Select All / Invert / Copy paths).

## Finding duplicates

Right-click a result and choose **Find duplicates in results…** (or, with rows
checked, **Find duplicates in selection**) to look for files with *identical
contents*. The scan runs on a background thread — the window reports how many
candidate files it is comparing — and results appear grouped, largest waste
first:

```
3 copies · 14.6 MiB each · 29.2 MiB wasted   [Keep newest] [Select all] [Select in results] [Copy paths]
   ☐ Solution Design Document - DMT V1.7.pdf   …/Workspace/SAB/SDD      [Reveal]
   ☐ Solution Design Document - DMT V1.7.pdf   …/OneDrive/Attachments   [Reveal]
   ☐ Solution Design Document - DMT V1.7.pdf   …/Downloads              [Reveal]
```

**Removing duplicates — to the Trash, never permanently.** Tick the copies you
no longer want (or use **Select all but one per group** / a group's **Keep
newest**, which keeps the most recently modified copy and ticks the rest) and
press **Move N to Trash…**. A confirmation lists exactly what will move; the
files then go to the desktop **Trash** (`~/.local/share/Trash`) with their
original path recorded, so any file manager can restore them. A whole group
being ticked raises a warning first, since that would remove every copy.

How it decides, and what it deliberately skips:

1. Only **files** are considered, and **empty files are ignored** (every empty
   file is trivially identical to every other — pure noise).
2. Files are grouped by **size** first, so unique sizes cost nothing.
3. Survivors are compared by **size + the first 64 KiB**.
4. Only those are **hashed in full** (SHA-256), and groups of more than one are
   reported. Two files that share a long prefix but differ later are therefore
   *not* reported as duplicates.
5. A scan looks at at most 50 000 candidates; if it hits that cap the window says
   **“scan capped”**.

Progress is cancellable-by-replacement: starting a new scan discards the old
one's result. The window can select, copy and reveal results — and **move
checked files to the Trash** (recoverable). The same **Move to Trash…** is on a
result row's right-click menu, for any selection.

## Tags

Tags are your own labels on files and folders — “work”, “2026”, “archive” —
stored locally in `~/.config/easysearch/tags.json` and shown as small `#chips`.
They are a view over the results, not part of the index: the engine never sees
them, and nothing is written to the tagged files themselves.

- **Add or remove them** from a result's right-click menu (**Tags…**). The editor
  acts on the whole checked selection when you opened it from a selected row, so
  one action can tag many files at once; the **Tag** quick action at the bottom
  of the preview pane opens it for the file you are looking at.
- **Filter** by a tag by clicking its chip (in a result row, in the preview, or
  in the status of the results header) or its row in the sidebar's **TAGS**
  section. The active tag is shown in the results header with a ✕ to clear it.
- The sidebar lists every tag in use with **how many paths carry it**; a tag on
  some-but-not-all of a multi-file selection is shown as `#tag (2/5)`.
- Tags follow **paths**, so renaming or moving a file outside EasySearch leaves
  the tag pointing at the old path (it simply stops matching). A store that
  cannot be read is ignored rather than blocking startup.

## Right-hand panel

- **Preview** — by file kind: image thumbnails for pictures; **audio/video**
  metadata plus a video first-frame; the **text layer** of PDF and office
  documents (`.docx`, `.odt`/`.ods`/`.odp`/`.odg`) extracted in-process;
  **rendered Markdown**; and **syntax-highlighted source code**. The highlighter
  covers the major languages (Rust, C/C++, Java, Go, Python, JS/TS, shell,
  JSON/YAML/TOML/INI, HTML/XML, CSS, SQL, Ruby, PHP, Lua, Kotlin, Swift, C#,
  R, Haskell, Scala, Dart, Perl, Nix, Elixir, Erlang, Clojure, …) and colours
  fenced code inside a Markdown preview too. It is a lightweight tokeniser —
  comments, strings, numbers, keywords, types and calls — not a parser; files
  over 128 KiB fall back to plain monospace. Folders and empty files get a short
  note instead.
- **Details** — Name · Path · Size (with exact bytes) · Modified · Created
  (filesystem birth time where the filesystem records one, else `—`) · MIME type
  (from the extension) · Permissions · **SHA-256** (computed on demand for files
  up to 512 MB, so opening Details on a huge file never stalls the UI).
- **Quick actions** — Open · Reveal · Copy path · Terminal.

The **view tabs** above the status bar switch the main area between *Results*,
*Preview*, *Details* and *Search History*.

## Status bar

Live index state (`Indexing: idle (86,100 files)` or a progress spinner),
system **CPU** and **RAM**, the number of results and the search time, the total
indexed entries, the **UI zoom** (100% / 110% / 125% — also in Settings), the
current keyboard hints, and — at the far right — the **running version**
(`v0.38.0`), so you can tell which build is installed. Hover any result's row for
its full path.

## Keyboard shortcuts

| Key | Action |
|---|---|
| `↑` `↓` `PgUp` `PgDn` | Navigate results |
| `Enter` | Open the selected file |
| Double-click | Open a result |
| `Esc` | Clear the search |
| `Ctrl+F` | Focus the search box |
| `Ctrl+A` | Select all result rows (when the search box is not focused) |
| Right-click a row | The full context menu (see *Results*) |
| `Ctrl+T` / `Ctrl+W` | New tab / close tab |
| `Ctrl+Tab` | Next tab |
| `Ctrl+1…9` | Select tab |
| `↑` `↓` in an empty search box | Cycle search history |

**Focus follows the pointer.** Clicking anywhere outside the search field drops
its keyboard focus, so `↑`/`↓`/`PgUp`/`PgDn`/`Enter` act on the results; the
first printable key you type then re-grabs the field and starts a new query, so
you never have to click back into it.

## Tray and closing

EasySearch keeps running when you close its window. The **X** button hides the
window — the engine child keeps indexing and the search stays warm — and the app
is only stopped by **File ▸ Quit EasySearch**, the tray's *Quit*, or
`easysearch --quit`.

- **Clicking the tray icon** shows the window if it is hidden and hides it if it
  is visible (the tray menu's *Show / Hide window* does the same).
- With **no tray** (a desktop without a StatusNotifier host) the window still
  hides to the background; bring it back with `easysearch --show`, bindable as a
  desktop shortcut (see *Global hotkey* below).
- One caveat: on a **Wayland** session a window cannot be unmapped, so “hidden”
  means the window is parked out of the way rather than truly gone. On X11 it is
  a normal hide.

## Global hotkey

Wayland deliberately has no global-hotkey API and every desktop invents its own,
so the app ships a portable mechanism instead of a privileged one: the running
window listens on a small socket and the command line drives it.

```sh
easysearch --toggle        # show the window if hidden, hide it if visible
easysearch --show          # bring it to the front
easysearch --hide
easysearch --search TODO   # open it and run a search
easysearch --quit          # stop the app and the engine
```

Bind one of those as a **custom shortcut** in your desktop:

- **KDE Plasma:** System Settings ▸ Shortcuts ▸ Custom ▸ *Edit* ▸ *New* ▸
  *Global Shortcut* ▸ *Command or Script* — `easysearch --toggle`.
- **GNOME:** Settings ▸ Keyboard ▸ Custom Shortcuts ▸ `+` —
  `easysearch --toggle`.
- **Sway / i3 / Hyprland:** `bindsym $mod+space exec easysearch --toggle`.

The first `--toggle`, `--show` or `--search` with nothing running **starts** the
app, so one key launches it and then drives it — a fresh `--search QUERY` runs
that query as soon as the window opens. `--hide` and `--quit` need a running
instance and quietly do nothing otherwise. The socket lives at
`$XDG_RUNTIME_DIR/easysearch.sock` with mode `0600`, so only your own user
can reach it.

Hiding (`--hide`) is not the same as quitting: the window is just put out of
sight, and the app and its engine keep running and indexing. *Quit*
(`easysearch --quit`) stops the app and the engine together.

## Theming and fonts

**The theme is your choice, remembered.** Settings ▸ Appearance (and
View ▸ Theme) offer **Dark** (the default), **Light**, and **Brand** — a deep
teal palette taken from the logo (teal accent, pink and lime highlights). The
choice is stored by name as `"theme"` in `gui.json`; an older `"dark":
true|false` (or `null`, the previous “follow system”) is still read, resolving to
Dark rather than resetting the file.

**Fonts come from the system.** The app resolves the desktop's configured family
with `fc-match` — KDE's `kdeglobals` (`font` / `fixed`), then GTK's
`gtk-font-name` / `gtk-monospace-font-name`, then fontconfig's `sans-serif` /
`monospace` — and draws with that, for both the UI and the monospace face (paths,
sizes). It does not ship its own font: the bundled egui faces remain only as glyph
fallbacks for what the system font lacks (emoji, CJK, rare symbols).

## Notes and limits

- A **0-byte file** shows “No text preview for this file” rather than an empty
  document.
- The **content index** (the optional document-text cache) is a live switch in
  **Settings ▸ Indexing** (*Background content index*). It is **off at boot**
  and, by default, **spooled to disk** (`<disk_index_dir>/content/`) rather than
  held in RAM, so it does not grow the resident set; use `--content-in-memory` for
  the RAM-backed mode. See [`config.md`](config.md).
- The **Fuzzy** and **Duplicate Finder** buttons from the reference design are
  deliberately absent rather than present-but-dead; so are Tags and Rename/Delete.
  [`pending.md`](pending.md) §4 lists them.
- The tray icon offers Open, Recent searches, and Quit EasySearch (which stops
  the app and the engine). It lives in the app process (with the window), so it is
  still there after the window is hidden.
- **Closing the window hides it to the tray**, not quitting the app: the engine
  keeps indexing while it is hidden. Use the **X** or *File ▸ Hide window* to hide
  it, and *Quit* (tray ▸ *Quit EasySearch*, *File ▸ Quit EasySearch*, or
  `easysearch --quit`) to stop the app and the engine. There is no separate
  "close to tray" or "quit on close" setting — the **X** always just hides.
  `easysearch --toggle` brings a hidden window back.
