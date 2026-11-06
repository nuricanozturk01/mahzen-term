# Config switcher (profiles)

A profile is a named set of environment variables and, optionally, a theme. Switch with the palette
(`Ctrl+Shift+P`, type "Profile"): one command per profile (`Profile: Work`), `Profile: None`, and
`Profile: Reload Profiles From Disk`. The status bar shows the active profile (the `profile` widget, hidden while none).

**New tabs and panes** start with the profile's environment. Shells that are already running are never changed behind
your back; run the `export` yourself there if you need it. Switching also applies the profile's `theme`.

Profiles live in `<config dir>/profiles/<id>.toml` (`~/.config/mahzenterm/profiles/` on Linux). The id is the file
name: `a-z`, `0-9`, `-`, `_`.

```toml
name = "Work"                          # shown in the palette and status bar (default: the id)
description = "Company cluster and git identity"
theme = "nord"                         # optional
[env]
AWS_PROFILE = "work"
KUBECONFIG = "~/.kube/work"            # a leading ~/ becomes your home folder
GIT_AUTHOR_EMAIL = "me@corp.example"
GIT_COMMITTER_EMAIL = "me@corp.example"
```

**Per project:** a file `.mahzenterm/profile` containing just a profile id (`work`) in a project folder (or any parent)
selects that profile when MahzenTerm starts in it. The file only *names* a profile from your own profiles folder; it
can never define variables, so opening a downloaded repository cannot inject an environment.

Limits and safety: files over 32 KiB are skipped, at most 64 variables per profile, 4 KiB per value. Variables that make
programs load foreign code (`LD_PRELOAD`, `LD_LIBRARY_PATH`, `LD_AUDIT`, `DYLD_INSERT_LIBRARIES`, `DYLD_LIBRARY_PATH`,
`BASH_ENV`, `ENV`) are rejected. Broken profiles are skipped and the first reason appears in the status bar.
The active profile is remembered in `state.toml`.

Not done yet: SSH config / kube context / git identity switching beyond environment variables, applying a profile to
running shells (an `export` suggestion), switching automatically on `cd` into a project (only at start-up for now).
