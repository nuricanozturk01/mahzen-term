//! Profiles (config switcher): loading them, switching, and the environment new panes start with.

use std::collections::HashMap;

use iced::Task;
use mahzenterm_config_switcher::{self as cs, Profile};
use mahzenterm_core::command::CommandSpec;

use super::{Message, Shell};

/// The folder profiles live in.
fn profiles_dir() -> Option<std::path::PathBuf> {
    mahzenterm_platform::config_dir().map(|d| d.join("profiles"))
}

impl Shell {
    /// Read profiles at start-up (one small folder) and pick the active one: a project's `.mahzenterm/profile`
    /// beats the one remembered in `state.toml`.
    pub(super) fn init_profiles(&mut self, remembered: Option<&str>) {
        let Some(dir) = profiles_dir() else {
            return;
        };
        let loaded = cs::load_dir(&dir);
        let from_project = std::env::current_dir()
            .ok()
            .and_then(|d| cs::project_profile(&d));
        self.set_profiles(loaded.profiles);
        if let Some(first) = loaded.problems.first() {
            self.problem = Some(format!("profile: {first}"));
        }
        let wanted = from_project.or_else(|| remembered.map(str::to_owned));
        if let Some(id) = wanted.filter(|id| self.profiles.iter().any(|p| &p.id == id)) {
            self.active_profile = Some(id);
        }
    }

    /// Replace the profile list and register a palette command for each new profile.
    pub(super) fn set_profiles(&mut self, profiles: Vec<Profile>) {
        let mut cmds: HashMap<String, String> = HashMap::new();
        for p in &profiles {
            let cmd = cs::use_command_id(&p.id);
            if cmds.insert(cmd.clone(), p.id.clone()).is_some() {
                tracing::warn!(profile = %p.id, "profile command id collides with another profile");
                continue;
            }
            if self.registry.get(&cmd).is_none() {
                let spec = CommandSpec::new(&cmd, &format!("Profile: {}", p.name), "Profile");
                if let Err(e) = self.registry.register(spec) {
                    tracing::warn!(error = %e, "cannot register profile command");
                }
            }
        }
        self.profile_cmds = cmds;
        self.profiles = profiles;
        if self
            .active_profile
            .as_ref()
            .is_some_and(|id| !self.profiles.iter().any(|p| &p.id == id))
        {
            self.active_profile = None;
        }
    }

    pub(super) fn active_profile_ref(&self) -> Option<&Profile> {
        let id = self.active_profile.as_ref()?;
        self.profiles.iter().find(|p| &p.id == id)
    }

    /// Environment variables new panes start with.
    pub(super) fn profile_env(&self) -> Vec<(String, String)> {
        self.active_profile_ref()
            .map(|p| p.env_for_spawn(mahzenterm_platform::home_dir().as_deref()))
            .unwrap_or_default()
    }

    /// Switch profile (`None` clears it). Running shells keep their environment; new tabs and panes get the new one.
    pub(super) fn activate_profile(&mut self, id: Option<String>) {
        let Some(id) = id else {
            self.active_profile = None;
            self.state_dirty = true;
            return;
        };
        let Some(theme) = self
            .profiles
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.theme.clone())
        else {
            self.problem = Some(format!(
                "profile {id} no longer exists; run \"Profile: Reload\""
            ));
            return;
        };
        self.active_profile = Some(id);
        self.state_dirty = true;
        if let Some(t) = theme {
            if self.themes.get(&t).is_some() {
                self.commit_theme(&t);
            } else {
                self.problem = Some(format!("profile theme \"{t}\" not found"));
            }
        }
    }

    /// `profile.reload`: read the folder again off the UI thread.
    pub(super) fn reload_profiles(&mut self) -> Task<Message> {
        let Some(dir) = profiles_dir() else {
            return Task::none();
        };
        Task::perform(async move { cs::load_dir(&dir) }, |l| {
            Message::ProfilesLoaded(l.profiles, l.problems)
        })
    }

    pub(super) fn on_profiles_loaded(&mut self, profiles: Vec<Profile>, problems: Vec<String>) {
        self.set_profiles(profiles);
        self.problem = problems.first().map(|p| format!("profile: {p}"));
    }
}
