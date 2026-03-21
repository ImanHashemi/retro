use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use std::io;

use crate::tui;

pub fn run(_verbose: bool) -> Result<()> {
    let dir = retro_dir();
    let db_path = dir.join("retro.db");
    let config_path = dir.join("config.toml");

    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let (cols, rows) = crossterm::terminal::size().context("getting terminal size")?;
    if cols < 60 || rows < 15 {
        anyhow::bail!("terminal too small (need at least 60x15, got {cols}x{rows})");
    }

    let config = Config::load(&config_path)?;
    let conn = db::open_db(&db_path)?;
    let mut app = tui::app::App::load(&conn, &config);

    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("entering alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("creating terminal")?;

    let result = run_event_loop(&mut terminal, &mut app, &conn);

    disable_raw_mode().context("disabling raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("leaving alternate screen")?;
    terminal.show_cursor().context("showing cursor")?;

    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut tui::app::App,
    conn: &db::Connection,
) -> Result<()> {
    loop {
        terminal.draw(|frame| tui::ui::draw(frame, app))?;
        if app.should_quit {
            return Ok(());
        }
        if event::poll(std::time::Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                tui::event::handle_key(app, key, conn);
            }
        }
    }
}
