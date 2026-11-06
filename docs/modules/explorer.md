# File explorer and `tf`

**Explorer** (`modules/explorer`): a tree dock on the left. Ctrl+B toggles it, Ctrl+Alt+B moves keyboard focus in
or out. It opens at the focused pane's directory. Folders are read lazily; `target`, `node_modules`,
`__pycache__`, `.git` and dotfiles are hidden until you press `.` (or the `.*` button).

| Key (dock focused) | Action |
|--------------------|--------|
| Up / Down / PgUp / PgDn / Home / End | Move selection |
| Right / Left | Open folder or step in / close folder or go to parent row |
| Enter / Space / double click | Toggle a folder; open a file in a preview pane (`tf view`, or `tf img` for images) |
| Backspace | Parent folder as root |
| `.` / `r` | Show hidden files / re-read from disk |
| `t` / `s` | New tab / split pane in the selected folder |
| `e` | Open the file in `$VISUAL` / `$EDITOR` in a split |
| `/` | Fuzzy file search (type, Enter opens, Esc leaves) |
| `n` / `N` | New file / new folder (in the selected folder) |
| F2 / Delete | Rename / move to the trash (asks first) |
| `q`, Esc | Close dock, leave the dock |

Changed files show a badge: `M` modified, `A` added, `R` renamed, `U` untracked, `D` deleted, `!` conflict; folders show
the most urgent change inside. Git runs in the background when the dock opens and on `r`. Names for new files and
renames are validated (no `/`, no `..`, never overwrites). Delete moves to the trash (freedesktop trash on Linux,
`~/.Trash` on macOS; Windows is not supported yet) and a file on another drive is refused, not copied.
File operations run on the UI thread (one syscall each).

Typed letters never reach the shell while the dock has focus; Ctrl/Alt shortcuts keep working.

**`tf`** (`crates/tf`, uses `modules/viewer`):

```
tf cat [--plain] [--width N] [--no-numbers] <file>...   render files (markdown, code, CSV/TSV)
tf view <file>...                                        same, in a pager ($PAGER, default `less -R`)
tf img [--width COLS] [--wait] <file>...                 draw images (see docs/graphics.md)
```

Markdown: headings, emphasis, lists (nested, tasks), quotes, fenced code with highlighting, tables, links,
rules. Code gets line numbers and light highlighting (Rust, Python, JS/TS, Go, C-family, shell, SQL, TOML,
YAML, JSON ...). Colours are the terminal's 16 ANSI slots, so every theme restyles the output. Colour is only
used on a terminal; pipes get plain text. Files over 8 MB are cut.

Both are modules under `modules/`: they implement `core::module::Module`, add their commands to the shared
registry (searchable in the palette, rebindable) and depend only on `core` and `ui-kit`.
