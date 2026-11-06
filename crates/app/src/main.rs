//! MahzenTerm application entry point.

mod chrome;
mod commands;
mod feed;
mod input;
mod layout;
mod live;
mod palette;
mod search;
mod shell;
mod state;

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
    let mut app = iced::application(Shell::boot, Shell::update, Shell::view)
        .title(Shell::title)
        .default_font(mahzenterm_ui_kit::fonts::MONO)
        .subscription(Shell::subscription)
        .window(iced::window::Settings {
            size: shell::INITIAL_SIZE,
            min_size: Some(iced::Size::new(520.0, 320.0)),
            decorations: !Shell::wants_custom_titlebar(),
            ..iced::window::Settings::default()
        });
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
