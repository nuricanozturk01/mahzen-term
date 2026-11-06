# 3. kubectl as the backend of the first Kubernetes slice

- Status: accepted (revisitable)
- Date: 2026-11-06

## Context
PLAN.md 5.6 names `kube-rs` for the Kubernetes module. `kube-rs` brings an async HTTP/TLS client stack (tokio, hyper,
rustls), a large amount of generated API types, and its own handling of kubeconfig auth plugins (exec credential
plugins for EKS, GKE, AKS, OIDC), which users already have working through `kubectl`.
CLAUDE.md asks to weigh maintenance, licence, build time and binary size before adding a dependency and not to
deviate silently from PLAN.md.

## Decision
The first slice drives the user's own `kubectl` for read-only queries (`-o json`) and runs logs/exec/describe in panes.
- `kubectl config view -o json` gives contexts without credentials (kubectl redacts them), so the app never touches
  tokens and needs no YAML parser.
- Auth plugins, proxies, certificate quirks and `KUBECONFIG` merging work exactly as for the user's terminal.
- Commands are argument vectors with validated names; every query has a 15 second timeout and runs off the UI thread.
- Destructive actions (delete, scale) are not offered. Switching to, or opening a shell on, a context whose name matches
  a production pattern asks first; the status bar and a top strip turn red.

## Consequences
- Requires `kubectl` on `PATH`; without it the panel says so in words.
- Per-query process start cost (tens of milliseconds), acceptable for a list the user asked for. Live watching (pod
  status streaming) is not possible this way; refresh is manual (`Ctrl+R`).
- The module talks to the backend only through `kubectl.rs` and `parse.rs`, so a `kube-rs` backend (for streaming and
  structured errors) can replace them later without touching the panel or the app. Revisit when live watch, port-forward
  management or in-app YAML editing is built.
