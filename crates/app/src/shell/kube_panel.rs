//! Kubernetes in the shell: context info for the status bar, the pods/contexts/namespaces panel, and logs, shells
//! and describe in panes. All `kubectl` queries run off the UI thread; logs and shells run in panes the user sees.

use iced::widget::{container, Space};
use iced::{Element, Length, Task};
use mahzenterm_core::command::Resolution;
use mahzenterm_core::workspace::PaneKind;
use mahzenterm_kubernetes as kube;
use mahzenterm_kubernetes::{Action, KubeConfig, Mode, Panel, Pod, Scope};

use super::{commands, Message, Shell};

/// Data a panel query returned.
#[derive(Debug, Clone)]
pub(crate) enum KubeData {
    Pods(Vec<Pod>),
    Namespaces(Vec<String>),
    Contexts(KubeConfig),
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// Run `kubectl` with `args` (blocking: only call inside a background task) and parse the answer.
fn query(program: &str, mode: Mode, args: Option<Vec<String>>) -> Result<KubeData, String> {
    let args = args.ok_or_else(|| "that name cannot be used on a command line".to_owned())?;
    let out = kube::run(program, &args)?;
    let bad = || "kubectl answered with something unexpected".to_owned();
    match mode {
        Mode::Pods => kube::parse_pods(&out, now_secs())
            .map(KubeData::Pods)
            .ok_or_else(bad),
        Mode::Namespaces => kube::parse_namespaces(&out)
            .map(KubeData::Namespaces)
            .ok_or_else(bad),
        Mode::Contexts => kube::parse_config(&out)
            .map(KubeData::Contexts)
            .ok_or_else(bad),
    }
}

fn map_key(
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
    text: Option<&str>,
) -> kube::Key<'static> {
    use iced::keyboard::key::Named;
    use iced::keyboard::Key;
    // `Text` borrows from the event; the panel copies it at once, so it is handled by the caller.
    let _ = text;
    match key {
        Key::Named(Named::Escape) => kube::Key::Escape,
        Key::Named(Named::Enter) => kube::Key::Enter,
        Key::Named(Named::ArrowUp) => kube::Key::Up,
        Key::Named(Named::ArrowDown) => kube::Key::Down,
        Key::Named(Named::PageUp) => kube::Key::PageUp,
        Key::Named(Named::PageDown) => kube::Key::PageDown,
        Key::Named(Named::Backspace) => kube::Key::Backspace,
        Key::Character(c) if modifiers.control() && !modifiers.alt() && !modifiers.logo() => {
            match c.to_lowercase().chars().next() {
                Some('u') => kube::Key::ClearLine,
                Some(ch) => kube::Key::Ctrl(ch),
                None => kube::Key::Other,
            }
        }
        _ => kube::Key::Other,
    }
}

impl Shell {
    /// Current context and namespace, when kubectl told us.
    fn kube_scope(&self) -> Option<Scope> {
        let cfg = self.kube_cfg.as_ref()?;
        let ctx = cfg.current_context()?;
        Some(Scope {
            context: ctx.name.clone(),
            namespace: cfg.current_namespace().to_owned(),
        })
    }

    /// Whether the current context is a production one.
    pub(super) fn kube_prod(&self) -> bool {
        self.kube_scope()
            .is_some_and(|s| self.kube_settings.is_prod(&s.context))
    }

    /// Read the kubeconfig summary once, in the background (and again after `kube.refresh` or a switch).
    pub(super) fn kube_task(&mut self) -> Task<Message> {
        if !self.kube_enabled || self.kube_asked {
            return Task::none();
        }
        self.kube_asked = true;
        let program = self.kube_settings.program().to_owned();
        Task::perform(
            async move { query(&program, Mode::Contexts, Some(kube::config_view())) },
            |r| {
                Message::KubeConfigLoaded(r.and_then(|d| match d {
                    KubeData::Contexts(c) => Ok(c),
                    _ => Err("unexpected data".into()),
                }))
            },
        )
    }

    pub(super) fn on_kube_config(&mut self, r: Result<KubeConfig, String>) {
        match r {
            Ok(cfg) => {
                self.kube_cfg = Some(cfg);
                self.kube_error = None;
            }
            Err(e) => {
                self.kube_cfg = None;
                self.kube_error = Some(e);
            }
        }
        // The header of an open panel follows the context.
        let (scope, prod) = (self.kube_scope(), self.kube_prod());
        if let Some(p) = self.kube_panel.as_mut() {
            p.scope = scope;
            p.prod = prod;
        }
    }

    /// `kube.refresh`.
    pub(super) fn refresh_kube(&mut self) -> Task<Message> {
        self.kube_enabled = true;
        self.kube_asked = false;
        self.kube_task()
    }

    pub(super) fn open_kube(&mut self, mode: Mode) -> Task<Message> {
        if self.kube_panel.take().is_some() && mode == Mode::Pods {
            return Task::none(); // the shortcut toggles it closed
        }
        self.kube_panel = Some(Panel::new(mode, self.kube_scope(), self.kube_prod()));
        self.kube_load(mode)
    }

    fn kube_load(&mut self, mode: Mode) -> Task<Message> {
        let scope = self.kube_scope();
        let program = self.kube_settings.program().to_owned();
        let args = match mode {
            Mode::Pods => scope.as_ref().and_then(kube::get_pods),
            Mode::Namespaces => scope
                .as_ref()
                .and_then(|s| kube::get_namespaces(&s.context)),
            Mode::Contexts => Some(kube::config_view()),
        };
        if args.is_none() {
            if let Some(p) = self.kube_panel.as_mut() {
                p.fail(self.kube_error.clone().unwrap_or_else(|| {
                    "no current Kubernetes context (is kubectl configured?)".into()
                }));
            }
            return Task::none();
        }
        Task::perform(async move { query(&program, mode, args) }, move |r| {
            Message::KubeLoaded(mode, r)
        })
    }

    pub(super) fn on_kube_loaded(&mut self, mode: Mode, r: Result<KubeData, String>) {
        let current_ns = self.kube_scope().map(|s| s.namespace).unwrap_or_default();
        let settings = self.kube_settings.clone();
        let Some(p) = self.kube_panel.as_mut().filter(|p| p.mode == mode) else {
            return; // a late answer for a list the user already left
        };
        match r {
            Ok(KubeData::Pods(pods)) => p.set_pods(&pods),
            Ok(KubeData::Namespaces(names)) => p.set_namespaces(&names, &current_ns),
            Ok(KubeData::Contexts(cfg)) => {
                p.set_contexts(&cfg, &|n| settings.is_prod(n));
                self.kube_cfg = Some(cfg);
            }
            Err(e) => p.fail(e),
        }
    }

    pub(super) fn kube_key(
        &mut self,
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
        text: Option<&str>,
    ) -> Task<Message> {
        if let Some(chord) = commands::chord_from(key, modifiers) {
            if self.keymap.resolve(&[chord], &|n| self.flag(n))
                == Resolution::Matched(kube::id::PODS.into())
            {
                self.kube_panel = None;
                return Task::none();
            }
        }
        let typed = text.filter(|t| {
            !modifiers.control()
                && !modifiers.alt()
                && !modifiers.logo()
                && !matches!(key, iced::keyboard::Key::Named(_))
                && !t.is_empty()
        });
        let Some(p) = self.kube_panel.as_mut() else {
            return Task::none();
        };
        let outcome = match typed {
            Some(t) => p.on_key(kube::Key::Text(t)),
            None => p.on_key(map_key(key, modifiers, text)),
        };
        self.apply_kube_outcome(outcome)
    }

    pub(super) fn kube_click(&mut self, row: usize) -> Task<Message> {
        let Some(p) = self.kube_panel.as_mut() else {
            return Task::none();
        };
        let outcome = p.click(row);
        self.apply_kube_outcome(outcome)
    }

    fn apply_kube_outcome(&mut self, outcome: kube::Outcome) -> Task<Message> {
        match outcome {
            kube::Outcome::Idle | kube::Outcome::Changed => Task::none(),
            kube::Outcome::Close => {
                self.kube_panel = None;
                Task::none()
            }
            kube::Outcome::Run(action) => self.run_kube_action(action),
        }
    }

    fn run_kube_action(&mut self, action: Action) -> Task<Message> {
        let scope = self.kube_scope();
        let program = self.kube_settings.program().to_owned();
        let cwd = self.focused_cwd();
        match action {
            Action::Load(mode) => return self.kube_load(mode),
            Action::UseContext(name) => {
                self.kube_panel = None;
                return self.kube_change(kube::use_context(&name));
            }
            Action::SetNamespace(ns) => {
                self.kube_panel = None;
                let args = scope.and_then(|s| kube::set_namespace(&s.context, &ns));
                return self.kube_change(args);
            }
            Action::Logs(pod) => {
                if let Some(args) = scope.as_ref().and_then(|s| kube::logs(s, &pod)) {
                    self.kube_panel = None;
                    self.split_with(PaneKind::Terminal {
                        cwd,
                        shell: Some(program),
                        args,
                    });
                }
            }
            Action::Exec(pod) => {
                if let Some(args) = scope.as_ref().and_then(|s| kube::exec_shell(s, &pod)) {
                    self.kube_panel = None;
                    self.window.new_tab(PaneKind::Terminal {
                        cwd,
                        shell: Some(program),
                        args,
                    });
                    self.sync();
                    self.invalidate_active();
                }
            }
            Action::Describe(pod) => {
                if let Some(args) = scope.as_ref().and_then(|s| kube::describe(s, &pod)) {
                    self.kube_panel = None;
                    // Positional parameters carry the command, so nothing is ever spliced into the script.
                    let mut argv = vec![
                        "-c".to_owned(),
                        "\"$@\" | ${PAGER:-less -R}".to_owned(),
                        "sh".to_owned(),
                        program,
                    ];
                    argv.extend(args);
                    self.window.new_tab(PaneKind::Terminal {
                        cwd,
                        shell: Some("sh".into()),
                        args: argv,
                    });
                    self.sync();
                    self.invalidate_active();
                }
            }
        }
        Task::none()
    }

    /// Run a kubeconfig-changing command in the background, then read the config again.
    fn kube_change(&mut self, args: Option<Vec<String>>) -> Task<Message> {
        let Some(args) = args else {
            self.problem = Some("that name cannot be used on a command line".into());
            return Task::none();
        };
        let program = self.kube_settings.program().to_owned();
        Task::perform(
            async move {
                kube::run(&program, &args)?;
                query(&program, Mode::Contexts, Some(kube::config_view()))
            },
            |r| {
                Message::KubeConfigLoaded(r.and_then(|d| match d {
                    KubeData::Contexts(c) => Ok(c),
                    _ => Err("unexpected data".into()),
                }))
            },
        )
    }

    /// The panel overlay, or `None` while closed.
    pub(super) fn kube_view(&self) -> Option<Element<'_, Message>> {
        let p = self.kube_panel.as_ref()?;
        Some(kube::view(
            p,
            self.tokens,
            Message::KubeRow,
            Message::KubeClose,
        ))
    }

    /// A thin red strip across the top while the current context is production.
    pub(super) fn prod_band(&self) -> Option<Element<'_, Message>> {
        if !self.kube_prod() {
            return None;
        }
        let danger = self.tokens.danger;
        Some(
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(3.0))
                .style(move |_| container::background(danger))
                .into(),
        )
    }

    /// Status-bar text for the `kube` widget: `context/namespace`.
    pub(super) fn kube_label(&self) -> Option<(String, bool)> {
        let s = self.kube_scope()?;
        Some((format!("{}/{}", s.context, s.namespace), self.kube_prod()))
    }
}
