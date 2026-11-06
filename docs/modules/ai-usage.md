# AI token usage card

`modules/ai-usage` shows how many tokens Claude Code and Codex used, in a small card that floats over the window.
`Ctrl+Alt+U` (`ai_usage.toggle`) shows or hides it. Drag the header (the strip with `TOKENS`) to put it anywhere;
the spot is remembered in `state.toml`.
Resize: drag the `◢` grip in the bottom-right corner, or press `Ctrl+Alt+=` / `Ctrl+Alt+-` (10 % per step, 60 %–250 %);
`ai_usage.reset_size` in the palette restores 100 %. The size is remembered too. `ai_usage.reset_position` (palette) sends it back to the configured corner.

Per tool the card shows: tokens spent today (input + output + cache writes), the last hour, the rate over the last
five minutes, and a today breakdown (`in`, `out`, `cache` reads). Cache reads are listed but not counted as spent.

Source: the tools' own local logs, read incrementally and de-duplicated by response id. Best effort: formats can
change with tool versions, and a missing tool simply shows zeros.

| Tool | Log read |
|------|----------|
| Claude Code | `$CLAUDE_CONFIG_DIR` or `~/.claude`: `projects/**/*.jsonl`, `assistant` lines with `message.usage` |
| Codex | `$CODEX_HOME` or `~/.codex`: `sessions/**/*.jsonl`, `token_usage_record` lines |

Nothing is sent anywhere. While the card is hidden the scan thread sleeps (no wakeups). No price estimate yet:
that needs an updatable `pricing.toml` (PLAN.md 5.5).

```toml
[modules.ai_usage]
visible = false          # show at startup (state.toml remembers the last choice)
anchor = "top_right"     # top_left | top_right | bottom_left | bottom_right
offset_x = 16.0
offset_y = 56.0
scale = 1.0              # initial size multiplier (0.6..2.5)
interval = 5.0           # seconds between scans while visible (1..60)
# claude_dir = "/custom/.claude"
# codex_dir = "/custom/.codex"
```

# Editor bridge

`modules/editor-bridge` opens files in your terminal editor at the right line. Ctrl+click a word such as
`src/main.rs:12:5` in any pane (compiler errors, grep output) and the file opens in a split. `e` in the explorer
and `Ctrl+Shift+O` (`editor.open_here`, the pane's folder) use it too.

Editor choice: `[modules.editor] command`, else `$VISUAL`, `$EDITOR`, else the first of `hx`, `nvim`, `micro`, `nano`
found on `PATH`. Line syntax per editor: Helix and micro `file:line:col`, Neovim `+line file`, nano `+line,col file`.
