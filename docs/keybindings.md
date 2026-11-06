# Default key bindings

Every action is a registered command (`crates/app/src/commands.rs`); rebind any of them in `config.toml`:

```toml
[[keys]]
keys = "Ctrl+Shift+N"
command = "tab.new"

[[keys]]            # remove a default
keys = "Ctrl+Shift+Q"
command = "-tab.close"
```

Config lives in `$XDG_CONFIG_HOME/mahzenterm/config.toml` (Linux), `~/Library/Application Support/mahzenterm/` (macOS),
`%APPDATA%\mahzenterm\` (Windows); a project can add `.mahzenterm/config.toml`.

| Action | Keys | Command id |
|---|---|---|
| New tab (opens in the current directory) | Ctrl+Shift+T | `tab.new` |
| Close tab | Ctrl+Shift+Q | `tab.close` |
| Next / previous tab | Ctrl+PageDown / Ctrl+PageUp | `tab.next` / `tab.prev` |
| Go to tab 1..9 | Alt+1 .. Alt+9 | `tab.goto_N` |
| Split right / down (inherits directory) | Ctrl+Shift+D / Ctrl+Shift+E | `pane.split_right` / `pane.split_down` |
| Close pane | Ctrl+Shift+W | `pane.close` |
| Zoom pane (toggle) | Ctrl+Shift+Enter | `pane.zoom` |
| Next / previous pane (wraps) | Ctrl+Alt+PageDown / Ctrl+Alt+PageUp | `pane.next` / `pane.prev` |
| Go to pane 1..9 | Ctrl+Alt+1 .. Ctrl+Alt+9 | `pane.goto_N` |
| Focus pane left/right/up/down | Ctrl+Alt+Arrows | `pane.focus_*` |
| Move pane border | Ctrl+Alt+Shift+Arrows | `pane.resize_*` |
| Swap pane with next | Ctrl+Alt+Shift+S | `pane.swap_next` |
| Copy / paste | Ctrl+Shift+C / Ctrl+Shift+V | `edit.copy` / `edit.paste` |
| Command palette (fuzzy search, themes preview live) | Ctrl+Shift+P | `palette.open` |
| Next / previous theme | (palette) | `theme.next` / `theme.prev` |
| Font larger / smaller / reset | Ctrl+= / Ctrl+- / Ctrl+0 | `view.font_larger` / `view.font_smaller` / `view.font_reset` |
| Full screen | F11 | `window.fullscreen` |
| Maximize (toggle) | (palette) | `window.maximize` |
| Quit | (palette) | `app.quit` |

Mouse: click a pane to focus it, drag a divider to resize, click a tab to switch.
Commands that do not apply (for example pane focus with a single pane) are inactive via their `when` condition.

## Palette, themes and state

- `Ctrl+Shift+P` opens the palette: type to filter, arrows move, Enter runs, Esc closes. Highlighting a "Color Theme"
  row previews it live on every pane; Esc restores the previous theme. Recently used commands come first.
- Themes are TOML files: built-in ones live in `themes/`, your own go in `<config dir>/themes/*.toml` (same format;
  an id equal to a built-in replaces it). Only terminal colours and one accent are listed; the chrome is derived from them.
- `state.toml` (next to `config.toml`) remembers the chosen theme, font size, recent commands and the tabs/splits with
  each pane's directory. Turn restore off with `[session] restore = false`. `[window] maximized = false` stops the
  window from starting maximized.
- Edits to `config.toml` apply live; a broken edit keeps the last valid settings and shows `file:line:col: message` in the
  status line.

## Search, links and clipboard
| Keys | Action |
|------|--------|
| Ctrl+Shift+F | Find in scrollback (a one-line selection becomes the query) |
| Enter / F3, Shift+Enter / Shift+F3 | Next / previous match; Esc closes |
| Alt+R | Toggle regular-expression search (the `.*` button) |
| Ctrl+Shift+Up / Down | Jump to previous / next prompt (needs shell integration: `docs/shell-integration.md`) |
| Ctrl+click (Cmd on macOS) | Open the link under the pointer (plain `http(s)`, `ftp`, `mailto` text and OSC 8 links) |

Search is plain text with smart case (a lowercase query ignores case). Only http, https, ftp and mailto
links open, one argument to the system opener, never through a shell. A program that asks to write the
clipboard (OSC 52) triggers an Allow / Deny prompt; set `[terminal] osc52 = "allow" | "deny" | "ask"`.
Reading the clipboard from a program is never possible. Pane headers have zoom and close buttons.
