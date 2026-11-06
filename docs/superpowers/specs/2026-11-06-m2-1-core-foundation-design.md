# M2.1 Core foundation — design

Status: approved by user ("devam"), 2026-11-06. Parent: PLAN.md section 10, M2.

## Goal
UI-free `mahzenterm-core`: event bus, command registry + keymap, layered config with live reload, workspace model.
All verified by unit/property tests without `iced`. `cargo xtask check-deps` stays clean.

## Scope split of M2
M2.1 core (this) · M2.2 theme engine · M2.3 multi-pane shell · M2.4 palette + status bar · M2.5 session restore.

## Modules

### bus
- `Command` (single consumer, bounded mpsc) and `Event` (broadcast). Both `#[non_exhaustive]`, serde-serializable.
- Commands: `OpenTab`, `ClosePane`, `SplitPane`, `FocusPane`, `RunInPane`, `ShowPanel`, `OpenFile{path,line}`, `Invoke{id,args}`.
- Events: `PaneSpawned`, `PaneExited`, `CwdChanged`, `TitleChanged`, `FocusChanged`, `ConfigReloaded`, `ThemeChanged`.
- Slow subscribers see `Lagged`, never crash the bus. Versioned `Envelope{schema, body}`.

### command
- `CommandSpec{id, title, category, default_keys, when}`; ids namespaced (`tab.new`).
- `when`: tiny expression language (`&&`, `||`, `!`, parentheses, identifiers) parsed once, evaluated against a context set.
- Keymap: key chords (incl. multi-step) to command ids; user overrides default; conflicts are reported, never silently dropped.
- One registry feeds palette, menus and shortcuts.

### config
- Layers: embedded default < user < project; deep merge by key. Typed, validated.
- Invalid file: keep last valid value, return `ConfigError` with line/column. Never panic.
- Live reload via `notify`, 150 ms debounce, runs off the UI thread, emits `ConfigReloaded`.
- `schema_version` with migrations. Modules register `[modules.<name>]` sections.

### workspace
- `Window -> Tab -> PaneTree`; node = `Split{axis, ratio, a, b}` | `Leaf(Pane)`; stable `PaneId(u64)`.
- `PaneKind`: `Terminal{cwd, shell}` | `Panel{module, panel}`.
- Model reserves `Tab.floating` (floating panes with rect) and `docks` (left/right/bottom panel lists) so the saved format
  does not change when those render later (inspired by session-HUD layouts: activity bar, side feeds, floating terminals).
- Pure operations: split, close, focus_dir, resize, zoom, move. Invariants: tree never empty, ratios clamped 0.1..0.9.

## Out of scope
`Module` trait (arrives with first module, M4), WASM, theming, rendering.

## Testing
Unit + `proptest` (when-parser, config merge, tree invariants) + `insta` (keymap resolution, config error messages).

## Dependencies (all MIT/Apache, maintained)
`serde`, `toml`, `thiserror`, `notify`, `tokio` (sync, rt, time), dev: `proptest`, `insta`.
