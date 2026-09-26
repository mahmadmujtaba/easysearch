# Screenshots

`dark.png` and `light.png` are the images the README and the AppStream metainfo
point at. Both show the same query (`2026`) in the two themes, at 1600 px wide.

They are **not** screenshots of a real home directory. They were taken from an
isolated EasySearch instance whose `$HOME` held a small throwaway demo tree
(`Documents/`, `Pictures/Screenshots/`, a few invoices, notes and source files),
so no personal filenames or paths are published.

## Re-taking them

After a UI change:

1. Build the app. Run it with `HOME`, `XDG_CONFIG_HOME` and `XDG_CACHE_HOME`
   pointing at a scratch directory that contains the demo tree — **not**
   `XDG_RUNTIME_DIR`, which is where the Wayland socket lives.
2. Select the theme through the config file rather than the menu, so the shot is
   reproducible: `$XDG_CONFIG_HOME/easysearch/gui.json` with `{"dark": true}` or
   `{"dark": false}`. Launch with a spare `--addr`, wait for
   `GET /v1/status` to report `"state":"Live"`, then drive the search over the
   control socket: `easysearch --addr 127.0.0.1:5959 --search 2026`.
3. Maximise and focus the window. On KDE a short script loaded through
   `qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.loadScript …` can do
   both; `spectacle -a` then grabs exactly that window:
   `spectacle -b -n -d 2 -a -o dark.png`.

`spectacle -a` follows the *active* window, so the app has to be active — if the
commands are being run from an editor's terminal, the editor is what gets
captured unless the app is explicitly activated first.
