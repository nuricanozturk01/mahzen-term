# Bars and widgets

The tab bar (top) and the status bar (bottom) are rows of **widgets** chosen in `config.toml`. Edits apply
instantly (live reload). A mistake keeps the last working layout and shows `config.toml:LINE:COL: ...` in the
status bar.

```toml
[tabbar]
left  = ["brand"]            # before the tabs
right = ["clock", "controls"] # after the tabs (controls = minimize / maximize / close)
tab_width = 176               # 90..=400
close_buttons = true
new_tab_button = true

[statusbar]
visible = true
left   = ["status", "pane", "tab", "zoom", "problem"]
center = []
right  = ["cpu", "memory", "clock", "theme"]

[window]
custom_titlebar = true   # the tab bar is the title bar; false = system frame (restart to change)
```

## Built-in widgets

| id | shows |
|----|-------|
| `brand` | logo and name |
| `status` | liveness dot and the focused pane's title or directory |
| `pane`, `tab`, `zoom` | `PANE 2/3`, `TAB 1/2`, `ZOOM` (only when relevant) |
| `problem` | config/theme problem (only while there is one) |
| `cpu`, `memory` | percentage with a gauge (green, amber over 70, red over 90) |
| `load`, `uptime` | one-minute load average, uptime |
| `clock` | time, `%H:%M` by default |
| `theme` | dot and the active theme name |
| `slogan` | the tagline |
| `separator`, `spacer` | thin line, 16 px gap |
| `controls` | window buttons (custom title bar only) |

## Your own widgets: `[widgets.<id>]`

`<id>` uses `a-z 0-9 _`. Put the id in any bar list. A definition with a built-in id overrides that built-in.

```toml
[widgets.news]            # scrolling text
kind = "text"
text = "Deploy window opens at 18:00  ·  "
marquee = true
width = 30                # visible characters
speed = 8                 # characters per second
color = "accent"
icon = "★"

[widgets.gpu]             # output of a command, refreshed every 5 s
kind = "command"
command = "nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader"
label = "GPU"
interval = 5
color = "ok"

[widgets.date]
kind = "clock"
format = "%a %d %b  %H:%M:%S"
on_click = "theme.next"   # any command id from the palette

[widgets.cpu]             # restyle a built-in
kind = "cpu"
style = "text"            # plain number, no gauge
interval = 1
```

| field | meaning |
|-------|---------|
| `kind` | `text`, `command`, or any built-in kind |
| `text` / `command` / `format` | content, shell line, `strftime` pattern |
| `label`, `icon` | small dim prefix, accent glyph |
| `color` | theme token: `accent` `text` `text_dim` `ok` `warn` `danger` |
| `interval` | seconds, 1..=86400 (`command`, `cpu`, `memory`, `load`, `uptime`) |
| `marquee`, `width`, `speed` | sideways scrolling in a window of `width` characters |
| `style` | `auto`, `text`, `chip` (bordered), `bar` (gauge) |
| `on_click` | command id run on click |

## Safety and cost

- **Commands run only from your own `config.toml`.** A `.mahzenterm/config.toml` inside a project may define
  text widgets and layouts, but its `command` widgets are dropped and reported, so cloning a repository can
  never run anything.
- A command is killed after 5 s; only its first output line (400 characters) is shown.
- Idle cost: bars without live widgets do nothing. The default bar wakes every 2 s for CPU and every 10 s for
  the clock. A scrolling marquee redraws about 30 times a second (about 2 % of one core).
- Custom title bar: drag empty bar space to move the window, double-click to maximize, drag the window edges
  to resize.

- `profile`: dot and the active profile's name (see `docs/modules/config-switcher.md`); hidden while no profile is active. In the default status bar, right group.
- `kube`: dot and `context/namespace` of the current Kubernetes context, red with a `PROD` tag for production contexts; hidden when kubectl reports none (see `docs/modules/kubernetes.md`). In the default status bar, right group.
