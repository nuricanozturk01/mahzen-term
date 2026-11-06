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

## M5 (partial) and extras
Done: `modules/editor-bridge` (Ctrl+click `file:line:col`, `e` in explorer, `editor.open_here`; vim removed from the
auto-detect list), `modules/ai-usage` (floating draggable Claude Code / Codex token card, `docs/modules/ai-usage.md`),
pane switching (`pane.next/prev`, `pane.goto_N`), explorer long names are ellipsized.
Not done: `modules/tui-store` (catalog, confirmed install, screenshots gallery), image protocols, price estimate.
PLAN.md gained 5.8 media (image viewer + video player, detachable to other windows/monitors) and rows M5b, M8b.

M5 tui-store done: embedded + user catalog, validated entries, confirmed visible install in a new tab, open installed apps
(`docs/modules/tui-store.md`). Remaining for M5: screenshots gallery, binary downloads, add-your-own form.

M4 completed (2026-11-06): git badges (background `git status`), fuzzy file search (`/`), new file/folder, rename, delete to
trash with confirmation (`platform::trash`), folder watcher (`platform::watch`, coalesced, zero idle cost). Explorer docs updated.
Still open in M4: image preview, syntax highlighting beyond the light built-in one.

M3 more done: OSC 7 working-directory reports, whole-word search (Alt+W). Still open in M3: Kitty graphics + Sixel images,
ligatures, IME.

M6 (first slice) done: `modules/ai-sessions` (live Claude Code sessions card, click to jump to the pane, `Ctrl+Alt+J` goes to
the one that needs me), `platform::proc` (process table), shared floating-card state (`shell/cards.rs`) used by the token
card too (`docs/modules/ai-sessions.md`). Not done in M6: Codex/other adapters, notifications, per-session cost, background host.

M7 (config-switcher) done: declarative profiles (env + theme), per-project `.mahzenterm/profile`, palette commands, status widget
`profile`, env applied to new panes (`docs/modules/config-switcher.md`). M7 kubernetes not started.

M7 kubernetes first slice done (via kubectl, ADR 0003): context/namespace widget with production warning, pods/contexts/namespaces
panel, logs/exec/describe in panes, production confirmations (`docs/modules/kubernetes.md`). Not done: live watch, port-forward,
YAML view/edit, delete/scale.

M3 images done (Kitty graphics + iTerm2 inline images, `docs/graphics.md`): verified in Xephyr with a real screenshot. Sixel, animation
and Kitty cropping/z-index are not done. Release binary 11 MB (image codecs). ~367 tests.

`tf img` done (any common image format, Kitty protocol; explorer previews images with it), verified with real screenshots.

Sixel done (`crates/term/src/sixel.rs`, bounded decoder). LICENSE-APACHE is now the full standard text. Known gaps left: CI never run
on GitHub, no git commits made (user asked for none), M8 / M8b / M9, ligatures, IME.
