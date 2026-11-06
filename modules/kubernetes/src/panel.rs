//! The Kubernetes panel's state machine: contexts, namespaces and pods as one searchable list, with a confirmation
//! step before anything that touches a production context. Pure data; the app runs the resulting [`Action`].

use mahzenterm_core::command::fuzzy_score;

use crate::kubectl::Scope;
use crate::parse::{age_label, Health, KubeConfig, Pod};

/// Rows shown at once.
pub const VISIBLE_ROWS: usize = 10;

/// What the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Pods,
    Contexts,
    Namespaces,
}

/// Colour class of a badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Pending,
    Bad,
    Neutral,
    Warn,
}

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The name the action acts on (pod, context or namespace).
    pub name: String,
    pub detail: String,
    pub badge: String,
    pub tone: Tone,
    /// A production context (shown in red, switching asks first).
    pub prod: bool,
}

/// Where the data stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Load {
    Loading,
    Ready,
    Failed(String),
}

/// Something to confirm before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    UseContext(String),
    Exec(String),
}

/// What the app should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    UseContext(String),
    SetNamespace(String),
    Logs(String),
    Exec(String),
    Describe(String),
    /// Load the list for this mode again (switching mode loads its data).
    Load(Mode),
}

/// What a key press did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Idle,
    Changed,
    Close,
    Run(Action),
}

/// A key press reduced to what the panel cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key<'a> {
    Escape,
    Enter,
    Up,
    Down,
    PageUp,
    PageDown,
    Backspace,
    ClearLine,
    /// Ctrl plus a letter (lowercase): `x` exec, `d` describe, `r` refresh, `n` namespaces, `k` contexts, `p` pods.
    Ctrl(char),
    Text(&'a str),
    Other,
}

/// An open panel.
#[derive(Debug)]
pub struct Panel {
    pub mode: Mode,
    pub items: Vec<Item>,
    pub query: String,
    pub results: Vec<usize>,
    pub selected: usize,
    pub load: Load,
    pub confirm: Option<Confirm>,
    /// Current context and namespace, for the header.
    pub scope: Option<Scope>,
    /// Whether the current context is a production one.
    pub prod: bool,
}

impl Panel {
    /// Open in `mode`, loading.
    pub fn new(mode: Mode, scope: Option<Scope>, prod: bool) -> Self {
        Self {
            mode,
            items: Vec::new(),
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            load: Load::Loading,
            confirm: None,
            scope,
            prod,
        }
    }

    pub fn current(&self) -> Option<&Item> {
        self.results.get(self.selected).map(|i| &self.items[*i])
    }

    pub fn first_visible(&self) -> usize {
        self.selected.saturating_sub(VISIBLE_ROWS - 1)
    }

    /// Switch to `mode` and wait for its data.
    pub fn switch(&mut self, mode: Mode) {
        self.mode = mode;
        self.items.clear();
        self.results.clear();
        self.query.clear();
        self.selected = 0;
        self.load = Load::Loading;
        self.confirm = None;
    }

    pub fn fail(&mut self, why: String) {
        self.items.clear();
        self.results.clear();
        self.load = Load::Failed(why);
    }

    pub fn set_pods(&mut self, pods: &[Pod]) {
        self.items = pods
            .iter()
            .map(|p| Item {
                name: p.name.clone(),
                detail: format!(
                    "{}/{} ready   {} restarts   {}",
                    p.ready,
                    p.total,
                    p.restarts,
                    age_label(p.age_secs)
                ),
                badge: p.status.clone(),
                tone: match p.health() {
                    Health::Good => Tone::Good,
                    Health::Pending => Tone::Pending,
                    Health::Bad => Tone::Bad,
                },
                prod: false,
            })
            .collect();
        self.ready();
    }

    pub fn set_contexts(&mut self, cfg: &KubeConfig, is_prod: &dyn Fn(&str) -> bool) {
        self.items = cfg
            .contexts
            .iter()
            .map(|c| {
                let current = cfg.current.as_deref() == Some(c.name.as_str());
                Item {
                    name: c.name.clone(),
                    detail: format!(
                        "{}  ns {}",
                        c.cluster,
                        c.namespace.as_deref().unwrap_or("default")
                    ),
                    badge: if current {
                        "current".into()
                    } else {
                        String::new()
                    },
                    tone: if current { Tone::Good } else { Tone::Neutral },
                    prod: is_prod(&c.name),
                }
            })
            .collect();
        self.ready();
    }

    pub fn set_namespaces(&mut self, names: &[String], current: &str) {
        self.items = names
            .iter()
            .map(|n| Item {
                name: n.clone(),
                detail: String::new(),
                badge: if n == current {
                    "current".into()
                } else {
                    String::new()
                },
                tone: if n == current {
                    Tone::Good
                } else {
                    Tone::Neutral
                },
                prod: false,
            })
            .collect();
        self.ready();
    }

    fn ready(&mut self) {
        self.load = Load::Ready;
        self.refilter();
    }

    fn refilter(&mut self) {
        let q = self.query.trim();
        if q.is_empty() {
            self.results = (0..self.items.len()).collect();
        } else {
            let mut scored: Vec<(i64, usize)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| fuzzy_score(q, &it.name).map(|s| (s, i)))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.results = scored.into_iter().map(|(_, i)| i).collect();
        }
        self.selected = 0;
    }

    fn move_by(&mut self, d: isize) {
        let n = self.results.len();
        if n > 0 {
            self.selected = (self.selected as isize + d).rem_euclid(n as isize) as usize;
        }
    }

    /// Enter, or a click on row `i`.
    pub fn activate(&mut self) -> Outcome {
        let Some(item) = self.current().cloned() else {
            return Outcome::Idle;
        };
        match self.mode {
            Mode::Pods => Outcome::Run(Action::Logs(item.name)),
            Mode::Namespaces => Outcome::Run(Action::SetNamespace(item.name)),
            Mode::Contexts if item.prod => {
                self.confirm = Some(Confirm::UseContext(item.name));
                Outcome::Changed
            }
            Mode::Contexts => Outcome::Run(Action::UseContext(item.name)),
        }
    }

    pub fn click(&mut self, i: usize) -> Outcome {
        if self.confirm.is_none() && i < self.results.len() {
            self.selected = i;
            return self.activate();
        }
        Outcome::Idle
    }

    pub fn on_key(&mut self, key: Key<'_>) -> Outcome {
        if let Some(c) = self.confirm.clone() {
            return match key {
                Key::Enter => {
                    self.confirm = None;
                    Outcome::Run(match c {
                        Confirm::UseContext(n) => Action::UseContext(n),
                        Confirm::Exec(p) => Action::Exec(p),
                    })
                }
                Key::Escape => {
                    self.confirm = None;
                    Outcome::Changed
                }
                _ => Outcome::Idle,
            };
        }
        match key {
            Key::Escape => Outcome::Close,
            Key::Enter => self.activate(),
            Key::Up => {
                self.move_by(-1);
                Outcome::Changed
            }
            Key::Down => {
                self.move_by(1);
                Outcome::Changed
            }
            Key::PageUp => {
                self.move_by(-(VISIBLE_ROWS as isize));
                Outcome::Changed
            }
            Key::PageDown => {
                self.move_by(VISIBLE_ROWS as isize);
                Outcome::Changed
            }
            Key::Backspace => {
                if self.query.pop().is_some() {
                    self.refilter();
                    Outcome::Changed
                } else {
                    Outcome::Idle
                }
            }
            Key::ClearLine => {
                if self.query.is_empty() {
                    Outcome::Idle
                } else {
                    self.query.clear();
                    self.refilter();
                    Outcome::Changed
                }
            }
            Key::Text(t) => {
                let clean: String = t.chars().filter(|c| !c.is_control()).collect();
                if clean.is_empty() {
                    return Outcome::Idle;
                }
                self.query.push_str(&clean);
                self.refilter();
                Outcome::Changed
            }
            Key::Ctrl(c) => self.ctrl(c),
            Key::Other => Outcome::Idle,
        }
    }

    fn ctrl(&mut self, c: char) -> Outcome {
        let pod = (self.mode == Mode::Pods)
            .then(|| self.current().map(|i| i.name.clone()))
            .flatten();
        match (c, pod) {
            ('x', Some(p)) if self.prod => {
                self.confirm = Some(Confirm::Exec(p));
                Outcome::Changed
            }
            ('x', Some(p)) => Outcome::Run(Action::Exec(p)),
            ('d', Some(p)) => Outcome::Run(Action::Describe(p)),
            ('r', _) => {
                let mode = self.mode;
                self.switch(mode);
                Outcome::Run(Action::Load(mode))
            }
            ('n', _) => self.go(Mode::Namespaces),
            ('k', _) => self.go(Mode::Contexts),
            ('p', _) => self.go(Mode::Pods),
            _ => Outcome::Idle,
        }
    }

    fn go(&mut self, mode: Mode) -> Outcome {
        self.switch(mode);
        Outcome::Run(Action::Load(mode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{parse_config, parse_pods};

    fn pods() -> Vec<Pod> {
        parse_pods(
            r#"{"items":[
              {"metadata":{"name":"web-1","namespace":"shop"},"spec":{"containers":[{}]},"status":{"phase":"Running","containerStatuses":[{"ready":true,"restartCount":0,"state":{"running":{}}}]}},
              {"metadata":{"name":"db-0","namespace":"shop"},"spec":{"containers":[{}]},"status":{"phase":"Running","containerStatuses":[{"ready":false,"restartCount":3,"state":{"waiting":{"reason":"CrashLoopBackOff"}}}]}}]}"#,
            0,
        )
        .unwrap()
    }

    fn scope() -> Option<Scope> {
        Some(Scope {
            context: "dev".into(),
            namespace: "shop".into(),
        })
    }

    #[test]
    fn pods_filter_and_enter_follows_logs() {
        let mut p = Panel::new(Mode::Pods, scope(), false);
        assert_eq!(p.load, Load::Loading);
        p.set_pods(&pods());
        assert_eq!(p.results.len(), 2);
        assert_eq!(p.items[1].tone, Tone::Bad);
        p.on_key(Key::Text("db"));
        assert_eq!(p.current().unwrap().name, "db-0");
        assert_eq!(
            p.on_key(Key::Enter),
            Outcome::Run(Action::Logs("db-0".into()))
        );
        assert_eq!(
            p.on_key(Key::Ctrl('d')),
            Outcome::Run(Action::Describe("db-0".into()))
        );
        assert_eq!(
            p.on_key(Key::Ctrl('x')),
            Outcome::Run(Action::Exec("db-0".into()))
        );
    }

    #[test]
    fn production_context_switches_and_execs_ask_first() {
        let cfg = parse_config(
            r#"{"current-context":"dev","contexts":[{"name":"dev","context":{"cluster":"k"}},{"name":"prod-eu","context":{"cluster":"e","namespace":"shop"}}]}"#,
        )
        .unwrap();
        let mut p = Panel::new(Mode::Contexts, scope(), false);
        p.set_contexts(&cfg, &|n| n.contains("prod"));
        assert_eq!(p.items[0].badge, "current");
        assert!(p.items[1].prod);
        // Switching to dev is immediate; to prod it asks.
        assert_eq!(
            p.on_key(Key::Enter),
            Outcome::Run(Action::UseContext("dev".into()))
        );
        p.on_key(Key::Down);
        assert_eq!(p.on_key(Key::Enter), Outcome::Changed);
        assert_eq!(p.confirm, Some(Confirm::UseContext("prod-eu".into())));
        assert_eq!(
            p.on_key(Key::Text("x")),
            Outcome::Idle,
            "typing is ignored while asking"
        );
        assert_eq!(p.on_key(Key::Escape), Outcome::Changed);
        assert!(p.confirm.is_none());
        p.on_key(Key::Enter);
        assert_eq!(
            p.on_key(Key::Enter),
            Outcome::Run(Action::UseContext("prod-eu".into()))
        );
        // Exec inside a production context asks too.
        let mut prod = Panel::new(Mode::Pods, scope(), true);
        prod.set_pods(&pods());
        assert_eq!(prod.on_key(Key::Ctrl('x')), Outcome::Changed);
        assert_eq!(prod.confirm, Some(Confirm::Exec("web-1".into())));
        assert_eq!(
            prod.on_key(Key::Enter),
            Outcome::Run(Action::Exec("web-1".into()))
        );
    }

    #[test]
    fn modes_switch_with_ctrl_keys_and_reload() {
        let mut p = Panel::new(Mode::Pods, scope(), false);
        p.set_pods(&pods());
        assert_eq!(
            p.on_key(Key::Ctrl('n')),
            Outcome::Run(Action::Load(Mode::Namespaces))
        );
        assert_eq!((p.mode, p.load.clone()), (Mode::Namespaces, Load::Loading));
        assert!(p.items.is_empty());
        p.set_namespaces(&["default".into(), "shop".into()], "shop");
        assert_eq!(p.items[1].badge, "current");
        assert_eq!(p.on_key(Key::Down), Outcome::Changed);
        assert_eq!(
            p.on_key(Key::Enter),
            Outcome::Run(Action::SetNamespace("shop".into()))
        );
        assert_eq!(
            p.on_key(Key::Ctrl('k')),
            Outcome::Run(Action::Load(Mode::Contexts))
        );
        assert_eq!(
            p.on_key(Key::Ctrl('r')),
            Outcome::Run(Action::Load(Mode::Contexts))
        );
        assert_eq!(
            p.on_key(Key::Ctrl('p')),
            Outcome::Run(Action::Load(Mode::Pods))
        );
        // Pod-only keys do nothing in other modes.
        p.switch(Mode::Contexts);
        assert_eq!(p.on_key(Key::Ctrl('x')), Outcome::Idle);
        p.fail("kubectl was not found".into());
        assert!(matches!(p.load, Load::Failed(_)));
        assert_eq!(p.on_key(Key::Escape), Outcome::Close);
    }

    #[test]
    fn clicks_act_like_enter_and_selection_wraps() {
        let mut p = Panel::new(Mode::Pods, scope(), false);
        p.set_pods(&pods());
        assert_eq!(p.click(1), Outcome::Run(Action::Logs("db-0".into())));
        assert_eq!(p.click(9), Outcome::Idle);
        assert_eq!(p.selected, 1, "the clicked row is selected");
        p.on_key(Key::Up);
        assert_eq!(p.selected, 0);
        p.on_key(Key::Up);
        assert_eq!(p.selected, 1, "wraps to the last row");
        p.on_key(Key::Down);
        assert_eq!(p.selected, 0, "and back to the first");
    }
}
