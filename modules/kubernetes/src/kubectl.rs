//! Building and running `kubectl` commands. Names that come from a cluster or kubeconfig are validated before they
//! reach an argument list; commands are plain argument vectors, never shell strings.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Longest a query may run before it is killed.
const TIMEOUT: Duration = Duration::from_secs(15);
/// Most output bytes read.
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

/// A name safe to pass as an argument: no control characters or spaces, not an option, bounded length.
pub fn safe_arg(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && !s.starts_with('-')
        && s.chars().all(|c| !c.is_control() && !c.is_whitespace())
}

/// The scope of a command: context and namespace, passed explicitly so a pane never depends on shared state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub context: String,
    pub namespace: String,
}

impl Scope {
    fn flags(&self) -> Option<Vec<String>> {
        (safe_arg(&self.context) && safe_arg(&self.namespace)).then(|| {
            vec![
                format!("--context={}", self.context),
                format!("--namespace={}", self.namespace),
            ]
        })
    }
}

fn argv(mut parts: Vec<String>, scope: &Scope) -> Option<Vec<String>> {
    parts.extend(scope.flags()?);
    Some(parts)
}

fn s(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

/// `kubectl config view -o json` (credentials are redacted by kubectl).
pub fn config_view() -> Vec<String> {
    s(&["config", "view", "-o", "json"])
}

/// `kubectl config use-context <name>`.
pub fn use_context(name: &str) -> Option<Vec<String>> {
    safe_arg(name).then(|| vec!["config".into(), "use-context".into(), name.into()])
}

/// `kubectl config set-context <context> --namespace=<ns>`.
pub fn set_namespace(context: &str, namespace: &str) -> Option<Vec<String>> {
    (safe_arg(context) && safe_arg(namespace)).then(|| {
        vec![
            "config".into(),
            "set-context".into(),
            context.into(),
            format!("--namespace={namespace}"),
        ]
    })
}

pub fn get_pods(scope: &Scope) -> Option<Vec<String>> {
    argv(s(&["get", "pods", "-o", "json"]), scope)
}

pub fn get_namespaces(context: &str) -> Option<Vec<String>> {
    safe_arg(context).then(|| {
        s(&[
            "get",
            "namespaces",
            "-o",
            "json",
            &format!("--context={context}"),
        ])
    })
}

/// Follow a pod's logs.
pub fn logs(scope: &Scope, pod: &str) -> Option<Vec<String>> {
    safe_arg(pod).then_some(())?;
    argv(
        vec!["logs".into(), "-f".into(), "--tail=200".into(), pod.into()],
        scope,
    )
}

/// An interactive shell in a pod (`sh`, which every common image has). The scope flags must come before `--`:
/// everything after it belongs to the command run inside the pod.
pub fn exec_shell(scope: &Scope, pod: &str) -> Option<Vec<String>> {
    safe_arg(pod).then_some(())?;
    let mut args = vec!["exec".to_owned(), "-it".to_owned(), pod.to_owned()];
    args.extend(scope.flags()?);
    args.extend(["--".to_owned(), "sh".to_owned()]);
    Some(args)
}

/// Delete one pod (its controller usually starts a new one).
pub fn delete_pod(scope: &Scope, pod: &str) -> Option<Vec<String>> {
    safe_arg(pod).then_some(())?;
    argv(vec!["delete".into(), "pod".into(), pod.into()], scope)
}

pub fn get_deployments(scope: &Scope) -> Option<Vec<String>> {
    argv(s(&["get", "deployments", "-o", "json"]), scope)
}

/// Set a deployment's replica count. The upper limit keeps a typo or a held key from asking for thousands.
pub fn scale_deployment(scope: &Scope, name: &str, replicas: u32) -> Option<Vec<String>> {
    (safe_arg(name) && replicas <= MAX_REPLICAS).then_some(())?;
    argv(
        vec![
            "scale".into(),
            "deployment".into(),
            name.into(),
            format!("--replicas={replicas}"),
        ],
        scope,
    )
}

/// Most replicas the panel will ask for.
pub const MAX_REPLICAS: u32 = 100;

/// Describe a pod in a pager-less dump.
pub fn describe(scope: &Scope, pod: &str) -> Option<Vec<String>> {
    safe_arg(pod).then_some(())?;
    argv(vec!["describe".into(), "pod".into(), pod.into()], scope)
}

/// Run `kubectl` with `args`, killing it after 15 seconds. Blocking: call off the UI thread.
/// Errors are phrased for the user.
pub fn run(program: &str, args: &[String]) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("{program} was not found on PATH; install it to use the Kubernetes panel")
            } else {
                format!("cannot start {program}: {e}")
            }
        })?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(o) = out.as_mut() {
            let _ = o.take(MAX_OUTPUT as u64).read_to_end(&mut buf);
        }
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(e) = err.as_mut() {
            let _ = e.take(64 * 1024).read_to_end(&mut buf);
        }
        buf
    });
    let end = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() >= end => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} took longer than 15 seconds; is the cluster reachable?"
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("{program}: {e}")),
        }
    };
    let stdout = reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    if status.success() {
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    } else {
        let msg = String::from_utf8_lossy(&stderr);
        let first = msg
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("failed");
        Err(first
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> Scope {
        Scope {
            context: "prod-eu".into(),
            namespace: "shop".into(),
        }
    }

    #[test]
    fn commands_carry_an_explicit_scope() {
        assert_eq!(
            get_pods(&scope()).unwrap(),
            [
                "get",
                "pods",
                "-o",
                "json",
                "--context=prod-eu",
                "--namespace=shop"
            ]
        );
        assert_eq!(
            logs(&scope(), "web-1").unwrap(),
            [
                "logs",
                "-f",
                "--tail=200",
                "web-1",
                "--context=prod-eu",
                "--namespace=shop"
            ]
        );
        assert_eq!(
            exec_shell(&scope(), "web-1").unwrap(),
            [
                "exec",
                "-it",
                "web-1",
                "--context=prod-eu",
                "--namespace=shop",
                "--",
                "sh"
            ]
        );
        assert_eq!(
            use_context("dev").unwrap(),
            ["config", "use-context", "dev"]
        );
        assert_eq!(
            set_namespace("dev", "shop").unwrap(),
            ["config", "set-context", "dev", "--namespace=shop"]
        );
    }

    #[test]
    fn delete_and_scale_are_explicit_and_bounded() {
        assert_eq!(
            delete_pod(&scope(), "web-1").unwrap(),
            [
                "delete",
                "pod",
                "web-1",
                "--context=prod-eu",
                "--namespace=shop"
            ]
        );
        assert_eq!(
            scale_deployment(&scope(), "api", 4).unwrap(),
            [
                "scale",
                "deployment",
                "api",
                "--replicas=4",
                "--context=prod-eu",
                "--namespace=shop"
            ]
        );
        assert!(
            scale_deployment(&scope(), "api", 0).is_some(),
            "scaling to zero is allowed (after a confirmation)"
        );
        assert!(scale_deployment(&scope(), "api", MAX_REPLICAS + 1).is_none());
        assert!(scale_deployment(&scope(), "-oyaml", 1).is_none());
        assert!(delete_pod(&scope(), "a b").is_none());
        assert_eq!(
            get_deployments(&scope()).unwrap(),
            [
                "get",
                "deployments",
                "-o",
                "json",
                "--context=prod-eu",
                "--namespace=shop"
            ]
        );
    }

    #[test]
    fn unsafe_names_never_become_arguments() {
        for bad in ["", "-oyaml", "a b", "a\nb", "x\u{1b}[0m", &"n".repeat(300)] {
            assert!(use_context(bad).is_none(), "{bad:?}");
            assert!(logs(&scope(), bad).is_none(), "{bad:?}");
            assert!(set_namespace("dev", bad).is_none(), "{bad:?}");
        }
        let evil = Scope {
            context: "--token=x".into(),
            namespace: "shop".into(),
        };
        assert!(get_pods(&evil).is_none());
        assert!(safe_arg("arn:aws:eks:eu-west-1:123456789012:cluster/prod"));
        assert!(safe_arg("gke_proj_europe-west1_cluster-1"));
    }

    #[test]
    fn running_reports_missing_programs_failures_and_timeouts_in_words() {
        let e = run("definitely-not-kubectl-xyz", &[]).unwrap_err();
        assert!(e.contains("was not found on PATH"), "{e}");
        let ok = run("sh", &["-c".into(), "printf hi".into()]).unwrap();
        assert_eq!(ok, "hi");
        let e = run(
            "sh",
            &[
                "-c".into(),
                "echo 'The connection was refused' >&2; exit 1".into(),
            ],
        )
        .unwrap_err();
        assert!(e.contains("connection was refused"), "{e}");
    }
}
