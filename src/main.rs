//! deck: terminal apps around Claude Code. `deck todo` is the daily todo and the
//! team's day in one pane; `deck claude` wraps Claude Code with panels;
//! `deck mcp` lets Claude manage the todo.
//!
//! Claude Code does the reading (Linear, Slack, Gmail, calendars) through the
//! daily-todo skill on your subscription; this app schedules it, draws what it
//! wrote, opens links and drafts Linear tickets for items that have none.

mod agenda;
mod app;
mod claude;
mod config;
mod connectors;
mod jobs;
mod keymap;
mod mcp;
mod menu;
mod ping;
mod schedule;
mod snapshot;
mod ui;

use app::App;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use std::io::stdout;
use std::time::Duration;

fn main() -> std::io::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("todo") => {}
        Some("claude") => std::process::exit(claude::run(std::env::args().skip(2).collect())?),
        Some("mcp") => return mcp::serve(),
        Some("--version") => {
            println!("deck {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        _ => {
            println!(
                "deck todo     the daily todo and the team's day in this terminal\ndeck claude   Claude Code with panels: session, changed files, pull requests\ndeck mcp      MCP server on stdio, for Claude\n\n? in deck todo lists its keys; ctrl+] in deck claude opens its dialog."
            );
            return Ok(());
        }
    }
    let config = config::Config::load();
    snapshot::set_link_bases(config.links.clone());
    snapshot::set_ticket_prefixes(config.ticket_prefixes.clone());
    let mut app = App::new(config);
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture)?;

    let result = (|| -> std::io::Result<()> {
        while !app.quit {
            app.tick();
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            if app.edit_requested {
                execute!(stdout(), DisableMouseCapture)?;
                ratatui::restore();
                app.edit_draft();
                terminal = ratatui::init();
                execute!(stdout(), EnableMouseCapture)?;
                terminal.clear()?;
                continue;
            }
            // Short poll: the spinners and the clock move without input.
            if event::poll(Duration::from_millis(200))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                    Event::Mouse(mouse) => app.on_mouse(mouse),
                    _ => {}
                }
            }
        }
        Ok(())
    })();

    app.shutdown();
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}
