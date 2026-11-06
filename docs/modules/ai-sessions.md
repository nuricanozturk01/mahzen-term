# AI sessions card

`modules/ai-sessions` lists the Claude Code sessions running on this machine, with their live status, in a small
floating card. `Ctrl+Alt+A` (`ai_sessions.toggle`) shows or hides it. Drag the header to move it, drag the `◢` corner to
resize it (or palette: "Enlarge/Shrink AI Sessions Card"); position and size are remembered.

Each row: a status dot and name, the status (`busy`, `idle`, `needs you`, `shell`), how long ago it changed, and the
folder. The header counts `busy / need you / idle`. **Click a row to jump to the pane that session runs in.**
`Ctrl+Alt+J` (`ai_sessions.focus_waiting`) jumps straight to the first session that needs you (for example a permission
prompt), from anywhere.

How it works:
- Status comes from Claude Code's own records, `~/.claude/sessions/<pid>.json` (or `$CLAUDE_CONFIG_DIR`). Only your
  interactive chats count; helper and spare processes are skipped, and records of dead processes are ignored.
- The pane is found by walking the process tree: the session's process and its ancestors are matched against each pane's
  shell. A session started outside MahzenTerm shows in the list but a click says it is not in one of these panes.
- Nothing is written into Claude's configuration, no hooks, no network. The files are an implementation detail of the
  tool and may change with its versions; the card then shows fewer or no sessions rather than failing.
- While the card is hidden the scan thread sleeps (no wakeups, no CPU).

Not done yet: Codex and other tools (they have no per-process status file; the plan is CPU/child-process based adapters),
notifications when a session starts waiting, cost per session, a persistent background host.

```toml
[modules.ai_sessions]
visible = false            # show at startup (state.toml remembers the last choice)
anchor = "bottom_left"     # top_left | top_right | bottom_left | bottom_right
offset_x = 16.0
offset_y = 44.0
scale = 1.0                # 0.6..2.5
interval = 2.0             # seconds between scans while visible (1..60)
# claude_dir = "/custom/.claude"
```
