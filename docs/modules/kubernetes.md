# Kubernetes

Needs `kubectl` on `PATH` (your own, with your own login plugins). The status bar's `kube` widget shows
`context/namespace` of the current context (hidden when kubectl has none); when the context name matches a production
pattern it turns red with a `PROD` tag and a red strip runs across the top of the window.

| Key / command | Action |
|---|---|
| `Ctrl+Alt+K` (`kube.pods`) | Pods of the current namespace (toggles) |
| `kube.contexts` (palette) | Switch context |
| `kube.namespaces` (palette) | Switch namespace of the current context |
| `kube.refresh` (palette) | Read the kubeconfig again (it is read at start and after a switch, not polled) |

Inside the panel: type to filter; Enter acts (pods: **follow logs** in a new split; contexts: switch; namespaces: select);
`Ctrl+X` opens a shell in the pod (new tab), `Ctrl+D` describes it (pager in a new tab), `Ctrl+R` reloads,
`Ctrl+N` / `Ctrl+K` / `Ctrl+P` jump to namespaces / contexts / pods, Esc closes. Pod rows show status (colour coded,
`CrashLoopBackOff` and friends are red), ready count, restarts and age.

**Production safety:** switching to a production context, and opening a shell while the current context is production,
ask `Enter yes / Esc no` first. Delete and scale are deliberately not offered.
Logs and shells run in panes with `--context` and `--namespace` given explicitly, so they keep working if you switch
context afterwards.

Switching a context runs `kubectl config use-context`, which (as with `kubectx`) changes your kubeconfig's current context
for every terminal. Credentials are never read: `kubectl config view` redacts them.

```toml
[modules.kubernetes]
kubectl = "kubectl"                          # a bare program name (no path)
prod_patterns = ["prod", "production", "prd"] # context names containing one of these (any case) are production
```

Why `kubectl` and not a client library: `docs/adr/0003-kubectl-backend-for-kubernetes.md`.
Not done yet: live pod watching, port-forward management, resource YAML view/edit, delete/scale with confirmation.
