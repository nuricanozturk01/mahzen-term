//! The Docker panel in the shell (Ctrl+Alt+J): containers with start/stop/restart/remove, logs in a pane beside and
//! a shell inside a container in a new tab.

use iced::keyboard::{key::Named, Key as IcedKey, Modifiers};
use iced::{Element, Task};
use mahzenterm_core::workspace::PaneKind;
use mahzenterm_docker as docker;

use super::{Message, Shell};

fn map_key(key: &IcedKey, modifiers: Modifiers, text: Option<&str>) -> Option<docker::Key> {
    Some(match key {
        IcedKey::Named(Named::ArrowUp) => docker::Key::Up,
        IcedKey::Named(Named::ArrowDown) => docker::Key::Down,
        IcedKey::Named(Named::Enter) => docker::Key::Enter,
        IcedKey::Named(Named::Escape) => docker::Key::Escape,
        _ => {
            let t = text.filter(|t| {
                !modifiers.control() && !modifiers.alt() && !t.chars().any(char::is_control)
            })?;
            docker::Key::Text(t.to_owned())
        }
    })
}

impl Shell {
    pub(super) fn open_docker_panel(&mut self) -> Task<Message> {
        self.palette = None;
        self.docker_panel = Some(docker::Panel::new());
        self.docker_reload()
    }

    fn docker_reload(&self) -> Task<Message> {
        let program = self.docker_program.clone();
        Task::perform(async move { docker::fetch(&program) }, Message::DockerList)
    }

    pub(super) fn on_docker_list(&mut self, r: Result<Vec<docker::Container>, String>) {
        if let Some(p) = self.docker_panel.as_mut() {
            p.set_containers(r);
        }
    }

    pub(super) fn on_docker_done(&mut self, r: Result<String, String>) -> Task<Message> {
        let Some(p) = self.docker_panel.as_mut() else {
            return Task::none();
        };
        p.finish(r);
        p.busy = true;
        self.docker_reload()
    }

    pub(super) fn docker_key(
        &mut self,
        key: &IcedKey,
        modifiers: Modifiers,
        text: Option<&str>,
    ) -> Task<Message> {
        let Some(k) = map_key(key, modifiers, text) else {
            return Task::none();
        };
        let Some(p) = self.docker_panel.as_mut() else {
            return Task::none();
        };
        match p.key(k) {
            docker::Outcome::None => Task::none(),
            docker::Outcome::Close => {
                self.docker_panel = None;
                Task::none()
            }
            docker::Outcome::Do(a) => self.docker_action(a),
        }
    }

    fn docker_action(&mut self, action: docker::Action) -> Task<Message> {
        let program = self.docker_program.clone();
        match action {
            docker::Action::Refresh => self.docker_reload(),
            docker::Action::Operate(op, id) => Task::perform(
                async move { docker::operate(&program, op, &id) },
                Message::DockerDone,
            ),
            docker::Action::Logs(id) => {
                if docker::safe_id(&id) {
                    self.docker_panel = None;
                    self.split_with(PaneKind::Terminal {
                        cwd: None,
                        shell: Some(program),
                        args: ["logs", "-f", "--tail", "200"]
                            .iter()
                            .map(|s| (*s).to_owned())
                            .chain([id])
                            .collect(),
                    });
                }
                Task::none()
            }
            docker::Action::Shell(id) => {
                if docker::safe_id(&id) {
                    self.docker_panel = None;
                    self.window.new_tab(PaneKind::Terminal {
                        cwd: None,
                        shell: Some(program),
                        args: ["exec", "-it"]
                            .iter()
                            .map(|s| (*s).to_owned())
                            .chain([id, "sh".to_owned()])
                            .collect(),
                    });
                    self.sync();
                    self.invalidate_active();
                }
                Task::none()
            }
        }
    }

    pub(super) fn docker_view(&self) -> Option<Element<'_, Message>> {
        let p = self.docker_panel.as_ref()?;
        Some(docker::view(p, self.tokens, Message::DockerClose))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(id: &str) -> docker::Container {
        docker::Container {
            id: id.into(),
            name: "web".into(),
            image: "i".into(),
            state: "running".into(),
            status: "Up".into(),
        }
    }

    #[test]
    fn logs_open_beside_and_shell_in_a_tab_with_a_safe_id() {
        let mut s = super::super::tests::shell();
        s.docker_panel = Some(docker::Panel::new());
        s.on_docker_list(Ok(vec![container("abcdef123456")]));
        let panes = s.window.active_tab().pane_ids().len();
        let _ = s.docker_action(docker::Action::Logs("abcdef123456".into()));
        assert!(s.docker_panel.is_none());
        assert_eq!(s.window.active_tab().pane_ids().len(), panes + 1);
        let tab = s.window.active_tab();
        let PaneKind::Terminal { args, .. } = &tab.pane(tab.focus).unwrap().kind else {
            panic!("terminal")
        };
        assert_eq!(args.last().unwrap(), "abcdef123456");
        assert_eq!(args[0], "logs");
        // An id that is not hexadecimal never reaches a command line.
        s.docker_panel = Some(docker::Panel::new());
        let tabs = s.window.tabs.len();
        let _ = s.docker_action(docker::Action::Shell("--privileged".into()));
        assert_eq!(s.window.tabs.len(), tabs);
        let _ = s.docker_action(docker::Action::Shell("abcdef123456".into()));
        assert_eq!(s.window.tabs.len(), tabs + 1);
    }

    #[test]
    fn keys_map_without_shortcuts() {
        assert_eq!(
            map_key(
                &IcedKey::Character("s".into()),
                Modifiers::empty(),
                Some("s")
            ),
            Some(docker::Key::Text("s".into()))
        );
        assert_eq!(
            map_key(&IcedKey::Character("s".into()), Modifiers::CTRL, Some("s")),
            None
        );
    }
}
