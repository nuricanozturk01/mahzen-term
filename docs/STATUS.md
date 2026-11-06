# Status (2026-11-06)

Done: M0 scaffold, M1 terminal (PTY, VT, keys, selection, scrollback, built-in box/block glyphs, CPU renderer by default).
Measured (release, idle): 18 MB RSS, 19 threads, 0% CPU, 7 MB binary. Parser 212 MB/s, PTY path 145 MB/s.
Gate: `cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace && cargo xtask check-deps && cargo deny check` (53 tests green).

M2 shell done (2026-11-06):
- M2.1 core: `core::{bus, command, config, workspace}`; spec docs/superpowers/specs/2026-11-06-m2-1-core-foundation-design.md.
- M2.2 themes: TOML theme engine (`ui-kit::themes`), 11 built-in themes in `themes/`, user themes dir, chrome derived from
  terminal colours with WCAG 4.5:1 text guaranteed by tests, live preview from the palette.
- M2.3 multi-pane shell: tabs, splits, draggable dividers, zoom, swap, cwd inheritance, mouse + keyboard.
- M2.4 command palette (fuzzy, recents, themes with swatches), status line (dot + tag chips, config problems).
- M2.5 session restore (`state.toml`, written off the UI thread, atomic) and live config reload.
- Look: JetBrains Mono bundled (OFL, checksums in `crates/ui-kit/assets/fonts/README.md`), deep-sea default theme with a teal
  accent, HUD-style tags, hairlines, corner brackets on the focused pane. Starts maximized; F11 full screen.
Renamed from TermForge to MahzenTerm ("A terminal with depth"); the repo directory is still `termforge`.
Quality gate: fmt, clippy -D warnings, 192 tests, check-deps, cargo deny all green. Idle 0 CPU ticks over 10 s with 4 panes,
~20 MB RSS (release).

Next: M3 rich terminal (OSC 133/7/8/52, search, URLs, Kitty graphics), then M4 explorer + viewer. See PLAN.md section 10.
The reference repo's HUD ideas (activity rail, session cards, metric bar, feeds) belong to M6 ai-sessions; the workspace
model already reserves docks and floating panes for them.

Known gaps: no mouse close/zoom buttons on panes (keyboard/palette only), user themes are read at startup only (config
reload does not rescan the themes dir), glow effects are not possible on the CPU renderer, vim/htop visual check pending,
LICENSE-APACHE placeholder, no git repo yet, CI never run.

## Bars and widgets
Tab bar doubles as a custom title bar; both bars are widget rows configured in `config.toml`
(`docs/widgets.md`): cpu, memory, load, uptime, clock, theme, text/marquee, command output, click actions.
Project configs cannot define command widgets. Verified in Xephyr (no window manager, so title-bar drag and
edge resize are untested on a real WM).

## M3 (first slice)
Done: scrollback search (highlight, count, next/prev), clickable URLs and OSC 8 links, OSC 52 with prompt, pane
zoom/close buttons. Also done: OSC 133 prompt jumping and `exit` widget (`docs/shell-integration.md`), regex search (Alt+R).
Not done: OSC 7 (cwd comes from /proc), image protocols (kitty/sixel), whole-word search.
Checked with `top` (alternate screen); vim is not installed here.

## M4 (first slice)
`core::module` contract; `modules/viewer` + `tf cat|view`; `modules/explorer` dock with preview and editor hand-off
(`docs/modules/explorer.md`). Not done: git status badges, fuzzy file search, file operations (rename/delete),
file watching (refresh is manual), images and syntax highlighting beyond the built-in light highlighter.
