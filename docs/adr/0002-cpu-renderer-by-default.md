# 0002. CPU renderer (tiny-skia) by default, GPU opt-in

- Status: accepted
- Date: 2026-11-06

## Context
Minimum resource use is a primary product goal (PLAN.md section 1). A terminal redraws only changed content, so GPU
throughput is not the bottleneck; memory and start-up cost are.

## Measurements (release build, Xephyr :9, idle, one shell)
| Renderer | RSS | Threads | Idle CPU | Binary |
|---|---|---|---|---|
| wgpu (Vulkan) | 193 MB | 96 | 0.0% | 12.6 MB |
| tiny-skia (CPU) | 18 MB | 19 | 0.0% | 7.0 MB |

Full 10k-line scrollback at 120 columns peaks at 31 MB RSS. Parser alone: 212 MB/s; through a PTY: ~145 MB/s.
wgpu numbers were taken under a software Vulkan path (Xephyr), so the GPU figure is an upper bound; a real driver
still maps large driver libraries.

## Decision
`iced` is built with `default-features = false` and the `tiny-skia` renderer. The app crate has a `gpu` feature
(`iced/wgpu`) for later image-heavy modules; it is off by default.

## Consequences
- ~10x lower memory, smaller binary, no GPU driver dependency (works over SSH X forwarding and in VMs).
- Full-window redraws cost CPU; mitigated by the geometry cache (redraw only on new output) and, later, damage rects.
- Kitty graphics/Sixel (M3) must work in the CPU path; revisit if profiling shows otherwise.
