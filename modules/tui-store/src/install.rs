//! Choosing how to install an app: the package manager that fits the platform, as an explicit command line.
//! Nothing runs here; the app shows the command and runs it in a pane only after the user confirms.

use std::ffi::OsStr;
use std::path::PathBuf;

use crate::catalog::App;

/// A way to install packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Apt,
    Pacman,
    Brew,
    Winget,
    CargoBinstall,
    CargoInstall,
}

impl Manager {
    pub fn label(self) -> &'static str {
        match self {
            Self::Apt => "apt",
            Self::Pacman => "pacman",
            Self::Brew => "Homebrew",
            Self::Winget => "winget",
            Self::CargoBinstall => "cargo binstall",
            Self::CargoInstall => "cargo install",
        }
    }
}

/// A concrete command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub manager: Manager,
    pub program: String,
    pub args: Vec<String>,
}

impl InstallPlan {
    /// The command as the user will see it (and as it will run).
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Whether the command asks for administrator rights (the terminal prompts for the password itself).
    pub fn uses_sudo(&self) -> bool {
        self.program == "sudo"
    }
}

/// First executable called `name` on `path_var`.
pub fn find_program(name: &str, path_var: &OsStr) -> Option<PathBuf> {
    let suffix = std::env::consts::EXE_SUFFIX;
    std::env::split_paths(path_var).find_map(|dir| {
        [name.to_owned(), format!("{name}{suffix}")]
            .into_iter()
            .map(|n| dir.join(n))
            .find(|p| p.is_file())
    })
}

/// Whether `app.command` is installed.
pub fn is_installed(app: &App, path_var: &OsStr) -> bool {
    find_program(&app.command, path_var).is_some()
}

/// Managers worth trying on `os`, best first.
fn order(os: &str) -> &'static [Manager] {
    match os {
        "linux" => &[
            Manager::Apt,
            Manager::Pacman,
            Manager::Brew,
            Manager::CargoBinstall,
            Manager::CargoInstall,
        ],
        "macos" => &[Manager::Brew, Manager::CargoBinstall, Manager::CargoInstall],
        "windows" => &[
            Manager::Winget,
            Manager::CargoBinstall,
            Manager::CargoInstall,
        ],
        _ => &[Manager::CargoBinstall, Manager::CargoInstall],
    }
}

fn available(m: Manager, path_var: &OsStr) -> bool {
    let probe = |n: &str| find_program(n, path_var).is_some();
    match m {
        Manager::Apt => probe("apt-get") || probe("apt"),
        Manager::Pacman => probe("pacman"),
        Manager::Brew => probe("brew"),
        Manager::Winget => probe("winget"),
        Manager::CargoInstall => probe("cargo"),
        Manager::CargoBinstall => probe("cargo") && probe("cargo-binstall"),
    }
}

fn plan_for(m: Manager, app: &App) -> Option<InstallPlan> {
    let s = |v: &str| v.to_owned();
    let (program, args, pkg): (&str, Vec<String>, &Option<String>) = match m {
        Manager::Apt => (
            "sudo",
            vec![s("apt"), s("install"), s("-y")],
            &app.install.apt,
        ),
        Manager::Pacman => (
            "sudo",
            vec![s("pacman"), s("-S"), s("--needed")],
            &app.install.pacman,
        ),
        Manager::Brew => ("brew", vec![s("install")], &app.install.brew),
        Manager::Winget => (
            "winget",
            vec![s("install"), s("-e"), s("--id")],
            &app.install.winget,
        ),
        Manager::CargoBinstall => ("cargo", vec![s("binstall"), s("-y")], &app.install.cargo),
        Manager::CargoInstall => (
            "cargo",
            vec![s("install"), s("--locked")],
            &app.install.cargo,
        ),
    };
    let pkg = pkg.as_ref()?;
    let mut args = args;
    args.push(pkg.clone());
    Some(InstallPlan {
        manager: m,
        program: program.to_owned(),
        args,
    })
}

/// The best install command for `app` on `os` given the tools found on `path_var`; `None` when nothing fits.
pub fn plan_install(app: &App, os: &str, path_var: &OsStr) -> Option<InstallPlan> {
    if !app.supports(os) {
        return None;
    }
    order(os)
        .iter()
        .filter(|m| available(**m, path_var))
        .find_map(|m| plan_for(*m, app))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::parse_app;
    use std::fs;

    fn app() -> App {
        parse_app(
            r#"
id = "demo"
name = "Demo"
summary = "x"
command = "demo"
[install]
apt = "demo-apt"
brew = "demo-brew"
cargo = "demo-crate"
winget = "Pub.Demo"
"#,
        )
        .unwrap()
    }

    fn path_with(tools: &[&str]) -> (tempfile::TempDir, std::ffi::OsString) {
        let d = tempfile::tempdir().unwrap();
        for t in tools {
            fs::write(d.path().join(t), "").unwrap();
        }
        let p = std::env::join_paths([d.path()]).unwrap();
        (d, p)
    }

    #[test]
    fn linux_prefers_apt_then_falls_back_to_cargo() {
        let (_d, p) = path_with(&["apt", "cargo", "cargo-binstall"]);
        let plan = plan_install(&app(), "linux", &p).unwrap();
        assert_eq!(plan.display(), "sudo apt install -y demo-apt");
        assert!(plan.uses_sudo());
        let (_d, p) = path_with(&["cargo", "cargo-binstall"]);
        assert_eq!(
            plan_install(&app(), "linux", &p).unwrap().display(),
            "cargo binstall -y demo-crate"
        );
        let (_d, p) = path_with(&["cargo"]);
        assert_eq!(
            plan_install(&app(), "linux", &p).unwrap().display(),
            "cargo install --locked demo-crate"
        );
    }

    #[test]
    fn each_platform_uses_its_own_manager() {
        let (_d, p) = path_with(&["brew", "winget", "apt"]);
        assert_eq!(
            plan_install(&app(), "macos", &p).unwrap().manager,
            Manager::Brew
        );
        assert_eq!(
            plan_install(&app(), "windows", &p).unwrap().display(),
            "winget install -e --id Pub.Demo"
        );
    }

    #[test]
    fn no_plan_without_tools_package_or_platform_support() {
        let (_d, empty) = path_with(&[]);
        assert!(plan_install(&app(), "linux", &empty).is_none());
        // A manager is present but the app has no package for it.
        let (_d, p) = path_with(&["pacman"]);
        assert!(plan_install(&app(), "linux", &p).is_none());
        let mut a = app();
        a.platforms.supported = vec!["linux".into()];
        let (_d, p) = path_with(&["brew"]);
        assert!(plan_install(&a, "macos", &p).is_none());
    }

    #[test]
    fn installed_means_the_command_is_on_the_path() {
        let (_d, p) = path_with(&["demo"]);
        assert!(is_installed(&app(), &p));
        assert!(!is_installed(&app(), std::ffi::OsStr::new("")));
    }
}
