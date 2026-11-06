# TUI store

`Ctrl+Alt+S` (`store.open`, palette: "Open TUI Store") opens a catalog of terminal apps: yazi, Helix, Neovim, micro,
btop, htop, bottom, ncdu, lazygit, GitUI, lazydocker, K9s, Glow, Zellij. Type to search (name, category, summary),
arrows move, Enter acts, Esc closes.

- **OPEN** badge: the program is on `PATH`; Enter starts it in a new tab.
- **INSTALL** badge: Enter shows the exact command and the package manager; Enter again runs it in a **new tab** so you
  watch it (and type the password if it uses `sudo`). Esc goes back. Nothing is downloaded or run without that second Enter.
- **N/A** badge: no supported package manager found for this system; the panel says so.

Install order: Linux `apt`, `pacman`, Homebrew, `cargo binstall`, `cargo install --locked`; macOS Homebrew then cargo;
Windows `winget` then cargo. The first manager that is installed and has a package for the app wins.
Direct binary downloads (HTTPS + sha256) are not implemented yet.

## Catalog files

An app is data, not code. Embedded entries live in `modules/tui-store/catalog/*.toml`; your own go in
`<config dir>/store/*.toml` (same format; the same `id` replaces a built-in; files over 64 KiB are skipped):

```toml
id = "yazi"
name = "Yazi"
summary = "Blazing fast terminal file manager"
categories = ["files"]
command = "yazi"            # bare program name, looked up on PATH
[install]                   # any of: apt, pacman, brew, winget, cargo
cargo = "yazi-fm"
brew = "yazi"
[platforms]                 # optional; empty means all
supported = ["linux", "macos", "windows"]
```

Catalog text is untrusted: ids, commands and package names are checked (no spaces, quotes, `$`, leading `-`) before they
can reach a command line. Broken entries are skipped and counted in the panel footer; the first reason shows in the
status line. The catalog is read off the UI thread when the store opens.

Not done yet: screenshots gallery (needs image display, M3), add-your-own form, remote catalog sources with signed updates.
