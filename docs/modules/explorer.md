# File explorer and `tf`

**Explorer** (`modules/explorer`): a tree dock on the left. Ctrl+B toggles it, Ctrl+Alt+B moves keyboard focus in
or out. It opens at the focused pane's directory. Folders are read lazily; `target`, `node_modules`,
`__pycache__`, `.git` and dotfiles are hidden until you press `.` (or the `.*` button).

| Key (dock focused) | Action |
|--------------------|--------|
| Up / Down / PgUp / PgDn / Home / End | Move selection |
| Right / Left | Open folder or step in / close folder or go to parent row |
| Enter / Space / double click | Toggle a folder; open a file in a preview pane (`tf view`) |
| Backspace | Parent folder as root |
| `.` / `r` | Show hidden files / re-read from disk |
| `t` / `s` | New tab / split pane in the selected folder |
| `e` | Open the file in `$VISUAL` / `$EDITOR` in a split |
| `q`, Esc | Close dock, leave the dock |

Typed letters never reach the shell while the dock has focus; Ctrl/Alt shortcuts keep working.

**`tf`** (`crates/tf`, uses `modules/viewer`):

```
tf cat [--plain] [--width N] [--no-numbers] <file>...   render files (markdown, code, CSV/TSV)
tf view <file>...                                        same, in a pager ($PAGER, default `less -R`)
```

Markdown: headings, emphasis, lists (nested, tasks), quotes, fenced code with highlighting, tables, links,
rules. Code gets line numbers and light highlighting (Rust, Python, JS/TS, Go, C-family, shell, SQL, TOML,
YAML, JSON ...). Colours are the terminal's 16 ANSI slots, so every theme restyles the output. Colour is only
used on a terminal; pipes get plain text. Files over 8 MB are cut.

Both are modules under `modules/`: they implement `core::module::Module`, add their commands to the shared
registry (searchable in the palette, rebindable) and depend only on `core` and `ui-kit`.
