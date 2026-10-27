pub mod app;
pub mod ui;

use std::io;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::scan::ScanOptions;
use crate::select::KeepStrategy;
use app::App;

pub fn run(opts: ScanOptions, keep: KeepStrategy) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::new(opts, keep);
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    loop {
        app.tick();
        terminal.draw(|frame| ui::draw(frame, &mut *app))?;
        if app.quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(120))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.on_key(key);
        }
    }
}
