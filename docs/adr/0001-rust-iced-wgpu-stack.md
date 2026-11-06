# 0001. Rust, Iced and wgpu as the UI stack

- Status: accepted
- Date: 2026-11-06

## Context
MahzenTerm must run on Linux, macOS and Windows, stay light on RAM/CPU, and offer rich visuals (images, charts)
plus a modular architecture (PLAN.md).

## Decision
Rust workspace. UI: Iced 0.14 (Elm-style message passing, wgpu renderer). Terminal: `alacritty_terminal` for VT state,
`portable-pty` for PTY/ConPTY, `cosmic-text` for shaping, custom wgpu cell renderer.

## Alternatives rejected
- Tauri/webview: highest RAM, contradicts the "light" goal.
- Fully custom wgpu widget layer: months of extra work for the shell UI.

## Consequences
- Iced's widget set is limited; missing widgets are built in `ui-kit`.
- Risk: Iced may not sustain terminal-grade rendering. Mitigation: prototype and measure in M1; the renderer sits behind
  a `ui-kit` abstraction so it can move to a dedicated wgpu surface (PLAN.md section 11).
