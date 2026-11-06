//! The Docker panel model: containers with one-key actions. Removing waits for a `y`; stopping and restarting a
//! container are reversible and run at once.

use crate::{Container, Op};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Enter,
    Escape,
    Text(String),
}

/// Something the app must do for the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Operate(Op, String),
    /// Follow the logs in a terminal pane.
    Logs(String),
    /// A shell inside the container, in a new tab.
    Shell(String),
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    None,
    Close,
    Do(Action),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    Remove(Container),
}

/// The panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub containers: Vec<Container>,
    pub selected: usize,
    pub confirm: Option<Confirm>,
    pub notice: Option<(String, bool)>,
    pub busy: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel {
    pub fn new() -> Self {
        Self {
            containers: Vec::new(),
            selected: 0,
            confirm: None,
            notice: None,
            busy: true,
        }
    }

    pub fn current(&self) -> Option<&Container> {
        self.containers.get(self.selected)
    }

    /// New list: running containers first, the selection stays on the same container when it is still there.
    pub fn set_containers(&mut self, result: Result<Vec<Container>, String>) {
        self.busy = false;
        match result {
            Ok(mut list) => {
                let keep = self.current().map(|c| c.id.clone());
                list.sort_by_key(|c| (!c.running(), c.name.clone()));
                self.selected = keep
                    .and_then(|id| list.iter().position(|c| c.id == id))
                    .unwrap_or(0);
                self.containers = list;
            }
            Err(e) => {
                self.containers.clear();
                self.notice = Some((e, true));
            }
        }
    }

    pub fn finish(&mut self, result: Result<String, String>) {
        self.confirm = None;
        self.notice = Some(match result {
            Ok(m) => (m, false),
            Err(e) => (e, true),
        });
    }

    fn start(&mut self, a: Action) -> Outcome {
        self.busy = true;
        Outcome::Do(a)
    }

    pub fn key(&mut self, key: Key) -> Outcome {
        if let Some(Confirm::Remove(c)) = self.confirm.take() {
            if key == Key::Text("y".into()) {
                return self.start(Action::Operate(Op::Remove, c.id));
            }
            self.notice = Some(("cancelled".into(), false));
            return Outcome::None;
        }
        if self.busy && key != Key::Escape {
            return Outcome::None;
        }
        let n = self.containers.len();
        let cur = self.current().cloned();
        match key {
            Key::Escape => return Outcome::Close,
            Key::Up if n > 0 => self.selected = (self.selected + n - 1) % n,
            Key::Down if n > 0 => self.selected = (self.selected + 1) % n,
            Key::Enter => {
                if let Some(c) = cur {
                    return Outcome::Do(Action::Logs(c.id));
                }
            }
            Key::Text(t) => {
                let Some(c) = cur else {
                    return if t == "r" {
                        self.start(Action::Refresh)
                    } else {
                        Outcome::None
                    };
                };
                match t.as_str() {
                    "s" => return self.start(Action::Operate(Op::Start, c.id)),
                    "x" => return self.start(Action::Operate(Op::Stop, c.id)),
                    "t" => return self.start(Action::Operate(Op::Restart, c.id)),
                    "d" => self.confirm = Some(Confirm::Remove(c)),
                    "l" => return Outcome::Do(Action::Logs(c.id)),
                    "e" => return Outcome::Do(Action::Shell(c.id)),
                    "r" => return self.start(Action::Refresh),
                    "q" => return Outcome::Close,
                    _ => {}
                }
            }
            _ => {}
        }
        Outcome::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, name: &str, state: &str) -> Container {
        Container {
            id: id.into(),
            name: name.into(),
            image: "i".into(),
            state: state.into(),
            status: String::new(),
        }
    }

    fn panel() -> Panel {
        let mut p = Panel::new();
        p.set_containers(Ok(vec![
            c("aaaaaa", "stopped", "exited"),
            c("bbbbbb", "web", "running"),
        ]));
        p
    }

    fn t(s: &str) -> Key {
        Key::Text(s.into())
    }

    #[test]
    fn running_containers_come_first_and_the_selection_follows_the_container() {
        let mut p = panel();
        assert_eq!(p.current().unwrap().name, "web");
        p.key(Key::Down);
        assert_eq!(p.current().unwrap().name, "stopped");
        p.set_containers(Ok(vec![
            c("cccccc", "new", "running"),
            c("aaaaaa", "stopped", "exited"),
        ]));
        assert_eq!(
            p.current().unwrap().name,
            "stopped",
            "same container, new position"
        );
    }

    #[test]
    fn simple_actions_run_at_once_and_one_at_a_time() {
        let mut p = panel();
        assert_eq!(
            p.key(t("x")),
            Outcome::Do(Action::Operate(Op::Stop, "bbbbbb".into()))
        );
        assert_eq!(p.key(t("s")), Outcome::None, "busy");
        p.finish(Ok("stopped".into()));
        p.busy = false;
        assert_eq!(
            p.key(t("t")),
            Outcome::Do(Action::Operate(Op::Restart, "bbbbbb".into()))
        );
        p.busy = false;
        assert_eq!(p.key(t("l")), Outcome::Do(Action::Logs("bbbbbb".into())));
        assert_eq!(p.key(t("e")), Outcome::Do(Action::Shell("bbbbbb".into())));
    }

    #[test]
    fn removing_needs_a_yes() {
        let mut p = panel();
        assert_eq!(p.key(t("d")), Outcome::None);
        assert!(p.confirm.is_some());
        assert_eq!(p.key(t("n")), Outcome::None);
        assert!(p.confirm.is_none(), "any other key cancels");
        p.key(t("d"));
        assert_eq!(
            p.key(t("y")),
            Outcome::Do(Action::Operate(Op::Remove, "bbbbbb".into()))
        );
    }

    #[test]
    fn an_empty_panel_still_refreshes_and_closes() {
        let mut p = Panel::new();
        p.set_containers(Ok(Vec::new()));
        assert_eq!(p.key(t("s")), Outcome::None);
        assert_eq!(p.key(t("r")), Outcome::Do(Action::Refresh));
        p.busy = false;
        assert_eq!(p.key(Key::Escape), Outcome::Close);
    }
}
