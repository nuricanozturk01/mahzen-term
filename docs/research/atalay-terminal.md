# atalay-terminal: reading notes (source read 2026-11-06, commit 2ff375c, MIT)

Not a general terminal: macOS AppKit/Swift dashboard for AI coding sessions, with a JS (Electron) Windows port.

## What it does (relevant to MahzenTerm)
- `Engine/RadarEngine.swift` (2.1k lines): watches `~/.claude/projects` and `~/.claude/sessions` (FSEvents, 0.2 s latency),
  tails transcript `.jsonl` incrementally, joins with the live registry (`sessions/<pid>.json`) and a process poll.
- Status model: registry `status` (busy/idle/waiting/shell) wins while the process is alive; otherwise "busy" if the
  last record is < 90 s old, else "ended".
- Other tools: `Proc.otherAIs` name table; status from CPU + child processes; Codex rollout files matched by `payload.cwd`;
  Antigravity read from its SQLite DB. Marked fragile by the author (upstream formats change).
- Persistence: `Detached.swift` runs every chat in a detached `screen -dmS atl-<id>` with its own screenrc; windows only attach.
  Windows port: C# "session host" holding ConPTY sessions (`windows/src/host.cs`).
- Pricing table by model prefix; cost = list price estimate.
- `TokenSavers.swift`: installs third-party hooks into users' tool settings (not adopted, see PLAN.md 5.5).

## Lessons
- Transcript parsing must be incremental, tolerant (skip unknown line types, strip images before JSON parse for speed).
- Compact/resume duplicates records under a new session id: dedupe by `uuid`.
- Two live processes can hold one conversation: the later `updatedAt` owns the card.
- Closed hosted backend (friends/levels) is irrelevant; nothing here needs a server.

## Caveats
- Formats are undocumented; version-sniff and degrade to "unknown" instead of failing.
- Read-only study. No code copied; reimplement from the observed formats.
