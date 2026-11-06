//! Parsing `kubectl ... -o json` output. The text comes from a cluster or a kubeconfig, so it is untrusted: every
//! field is optional and lengths are limited before anything is shown or put in a command line.

use serde_json::Value;

/// Longest name kept (Kubernetes names are at most 253 characters).
const MAX_NAME: usize = 253;
/// Most items kept from a list.
const MAX_ITEMS: usize = 2000;

/// One kubeconfig context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub name: String,
    pub cluster: String,
    pub namespace: Option<String>,
}

/// The parts of a kubeconfig the UI needs (never credentials: `kubectl config view` redacts them).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KubeConfig {
    pub current: Option<String>,
    pub contexts: Vec<Context>,
}

impl KubeConfig {
    pub fn current_context(&self) -> Option<&Context> {
        let cur = self.current.as_deref()?;
        self.contexts.iter().find(|c| c.name == cur)
    }

    /// Namespace of the current context (`default` when it sets none).
    pub fn current_namespace(&self) -> &str {
        self.current_context()
            .and_then(|c| c.namespace.as_deref())
            .unwrap_or("default")
    }
}

/// One pod as `kubectl get pods` would summarise it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pod {
    pub name: String,
    pub namespace: String,
    /// `Running`, `Pending`, `CrashLoopBackOff`, `Terminating`, ...
    pub status: String,
    pub ready: u32,
    pub total: u32,
    pub restarts: u32,
    /// Seconds since creation, when known.
    pub age_secs: Option<i64>,
}

/// How a pod's status should be coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Good,
    Pending,
    Bad,
}

impl Pod {
    pub fn health(&self) -> Health {
        match self.status.as_str() {
            "Running" if self.ready == self.total => Health::Good,
            "Succeeded" | "Completed" => Health::Good,
            "Running" | "Pending" | "ContainerCreating" | "PodInitializing" | "Terminating" => {
                Health::Pending
            }
            s if s.starts_with("Init:") && !s.contains("Error") && !s.contains("BackOff") => {
                Health::Pending
            }
            _ => Health::Bad,
        }
    }
}

fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect()
}

fn text(v: &Value, path: &[&str]) -> Option<String> {
    let mut cur = v;
    for k in path {
        cur = cur.get(k)?;
    }
    cur.as_str().map(clean).filter(|s| !s.is_empty())
}

/// Parse `kubectl config view -o json`.
pub fn parse_config(json: &str) -> Option<KubeConfig> {
    let v: Value = serde_json::from_str(json).ok()?;
    let contexts = v
        .get("contexts")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(MAX_ITEMS)
                .filter_map(|c| {
                    Some(Context {
                        name: text(c, &["name"])?,
                        cluster: text(c, &["context", "cluster"]).unwrap_or_default(),
                        namespace: text(c, &["context", "namespace"]),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(KubeConfig {
        current: text(&v, &["current-context"]),
        contexts,
    })
}

fn pod_status(p: &Value) -> (String, u32, u32, u32) {
    let statuses = p
        .pointer("/status/containerStatuses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total = u32::try_from(
        p.pointer("/spec/containers")
            .and_then(Value::as_array)
            .map_or(statuses.len(), Vec::len),
    )
    .unwrap_or(0);
    let ready = u32::try_from(
        statuses
            .iter()
            .filter(|s| s.get("ready").and_then(Value::as_bool) == Some(true))
            .count(),
    )
    .unwrap_or(0);
    let restarts = statuses
        .iter()
        .filter_map(|s| s.get("restartCount").and_then(Value::as_u64))
        .sum::<u64>()
        .min(u64::from(u32::MAX)) as u32;
    let mut status = text(p, &["status", "reason"])
        .or_else(|| text(p, &["status", "phase"]))
        .unwrap_or_else(|| "Unknown".into());
    for s in &statuses {
        if let Some(r) = text(s, &["state", "waiting", "reason"]) {
            status = r;
            break;
        }
        if let Some(r) = text(s, &["state", "terminated", "reason"]) {
            if status == "Running" || status == "Pending" || status == "Failed" {
                status = r;
            }
        }
    }
    if p.pointer("/metadata/deletionTimestamp").is_some() {
        status = "Terminating".into();
    }
    (status, ready, total, restarts)
}

/// Parse `kubectl get pods -o json`; `now` is unix seconds, used for ages.
pub fn parse_pods(json: &str, now: i64) -> Option<Vec<Pod>> {
    let v: Value = serde_json::from_str(json).ok()?;
    let items = v.get("items")?.as_array()?;
    Some(
        items
            .iter()
            .take(MAX_ITEMS)
            .filter_map(|p| {
                let (status, ready, total, restarts) = pod_status(p);
                let age_secs = text(p, &["metadata", "creationTimestamp"])
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
                    .map(|d| (now - d.timestamp()).max(0));
                Some(Pod {
                    name: text(p, &["metadata", "name"])?,
                    namespace: text(p, &["metadata", "namespace"]).unwrap_or_default(),
                    status,
                    ready,
                    total,
                    restarts,
                    age_secs,
                })
            })
            .collect(),
    )
}

/// Parse `kubectl get namespaces -o json` into names.
pub fn parse_namespaces(json: &str) -> Option<Vec<String>> {
    let v: Value = serde_json::from_str(json).ok()?;
    let mut names: Vec<String> = v
        .get("items")?
        .as_array()?
        .iter()
        .take(MAX_ITEMS)
        .filter_map(|n| text(n, &["metadata", "name"]))
        .collect();
    names.sort();
    Some(names)
}

/// `7200` -> `2h`, kubectl style (the largest unit only).
pub fn age_label(secs: Option<i64>) -> String {
    match secs {
        None => String::new(),
        Some(s) if s < 60 => format!("{s}s"),
        Some(s) if s < 3600 => format!("{}m", s / 60),
        Some(s) if s < 86_400 => format!("{}h", s / 3600),
        Some(s) => format!("{}d", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_lists_contexts_and_the_current_namespace() {
        let c = parse_config(
            r#"{"current-context":"prod-eu","contexts":[
              {"name":"dev","context":{"cluster":"kind","user":"u"}},
              {"name":"prod-eu","context":{"cluster":"eu-1","namespace":"shop","user":"u"}}]}"#,
        )
        .unwrap();
        assert_eq!(c.current.as_deref(), Some("prod-eu"));
        assert_eq!(c.contexts.len(), 2);
        assert_eq!(c.current_namespace(), "shop");
        assert_eq!(c.contexts[0].namespace, None);
        let none =
            parse_config(r#"{"contexts":[{"name":"a","context":{"cluster":"x"}}]}"#).unwrap();
        assert_eq!(none.current_context(), None);
        assert_eq!(none.current_namespace(), "default");
        assert!(parse_config("not json").is_none());
        assert_eq!(parse_config("{}").unwrap(), KubeConfig::default());
    }

    #[test]
    fn hostile_names_are_cleaned() {
        let c =
            parse_config("{\"contexts\":[{\"name\":\"a\\u001b[31mb\",\"context\":{}}]}").unwrap();
        assert_eq!(
            c.contexts[0].name, "a[31mb",
            "control characters are dropped"
        );
        let long = "n".repeat(500);
        let c = parse_config(&format!(
            "{{\"contexts\":[{{\"name\":\"{long}\",\"context\":{{}}}}]}}"
        ))
        .unwrap();
        assert_eq!(c.contexts[0].name.len(), MAX_NAME);
    }

    const PODS: &str = r#"{"items":[
      {"metadata":{"name":"web-1","namespace":"shop","creationTimestamp":"2026-11-06T10:00:00Z"},
       "spec":{"containers":[{},{}]},
       "status":{"phase":"Running","containerStatuses":[
         {"ready":true,"restartCount":0,"state":{"running":{}}},
         {"ready":true,"restartCount":2,"state":{"running":{}}}]}},
      {"metadata":{"name":"job-9","namespace":"shop"},
       "spec":{"containers":[{}]},
       "status":{"phase":"Running","containerStatuses":[
         {"ready":false,"restartCount":14,"state":{"waiting":{"reason":"CrashLoopBackOff"}}}]}},
      {"metadata":{"name":"old","namespace":"shop","deletionTimestamp":"2026-11-06T11:00:00Z"},
       "spec":{"containers":[{}]},"status":{"phase":"Running","containerStatuses":[{"ready":true,"restartCount":0,"state":{"running":{}}}]}},
      {"metadata":{"name":"done","namespace":"shop"},"spec":{"containers":[{}]},
       "status":{"phase":"Succeeded","containerStatuses":[{"ready":false,"restartCount":0,"state":{"terminated":{"reason":"Completed"}}}]}},
      {"metadata":{"name":"evicted","namespace":"shop"},"status":{"phase":"Failed","reason":"Evicted"}},
      {"spec":{}}
    ]}"#;

    #[test]
    fn pods_are_summarised_like_kubectl() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-06T12:00:00Z")
            .unwrap()
            .timestamp();
        let pods = parse_pods(PODS, now).unwrap();
        assert_eq!(pods.len(), 5, "an item without a name is skipped");
        let by = |n: &str| pods.iter().find(|p| p.name == n).unwrap();
        let web = by("web-1");
        assert_eq!(
            (web.status.as_str(), web.ready, web.total, web.restarts),
            ("Running", 2, 2, 2)
        );
        assert_eq!(web.age_secs, Some(7200));
        assert_eq!(web.health(), Health::Good);
        let job = by("job-9");
        assert_eq!(
            (job.status.as_str(), job.ready, job.total, job.restarts),
            ("CrashLoopBackOff", 0, 1, 14)
        );
        assert_eq!(job.health(), Health::Bad);
        assert_eq!(by("old").status, "Terminating");
        assert_eq!(by("old").health(), Health::Pending);
        assert_eq!(by("done").status, "Succeeded");
        assert_eq!(by("done").health(), Health::Good);
        assert_eq!(by("evicted").status, "Evicted");
        assert_eq!(by("evicted").health(), Health::Bad);
        assert!(parse_pods("{}", 0).is_none(), "no items list");
    }

    #[test]
    fn namespaces_sorted_and_ages_compact() {
        let ns = parse_namespaces(
            r#"{"items":[{"metadata":{"name":"zeta"}},{"metadata":{"name":"alpha"}},{}]}"#,
        )
        .unwrap();
        assert_eq!(ns, ["alpha", "zeta"]);
        assert_eq!(age_label(Some(5)), "5s");
        assert_eq!(age_label(Some(3 * 3600 + 10)), "3h");
        assert_eq!(age_label(Some(5 * 86_400)), "5d");
        assert_eq!(age_label(None), "");
    }
}
