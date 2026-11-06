//! MahzenTerm application entry point.

mod chrome;
mod commands;
mod feed;
mod input;
mod keyfile;
mod layout;
mod live;
mod palette;
mod search;
mod sessions;
mod shell;
mod state;
mod usage;

use std::process::ExitCode;

use shell::Shell;

fn run() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!(
            "{} {} - {}",
            mahzenterm_core::NAME,
            mahzenterm_core::VERSION,
            mahzenterm_core::SLOGAN
        );
        return Ok(());
    }
    mahzenterm_platform::check_display()?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    // A daemon, not a single-window application: surfaces can open in extra windows (another monitor), and the
    // main window is opened by `Shell::boot`.
    let mut app = iced::daemon(Shell::boot, Shell::update, Shell::view)
        .title(Shell::title)
        .default_font(mahzenterm_ui_kit::fonts::MONO)
        .subscription(Shell::subscription);
    for face in mahzenterm_ui_kit::fonts::all() {
        app = app.font(face);
    }
    app.run()?;
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mahzenterm: {e:#}");
            ExitCode::FAILURE
        }
    }
}
