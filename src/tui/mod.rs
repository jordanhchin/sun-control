//! Interactive terminal UI.

mod app;
mod logs;
mod ui;
mod widgets;

use std::time::Duration;

use crossterm::event::EventStream;
use futures::StreamExt;
use sun_control::connect::{ConnectError, ConnectOptions, Connection};
use sun_control::profiles::Profiles;
use tokio::sync::mpsc;

use app::App;

/// The initial connection is made before the TUI starts so prompts can use
/// the plain terminal; its outcome is passed in here.
pub async fn run(
    profiles: Profiles,
    opts: &ConnectOptions,
    initial: Result<Connection, ConnectError>,
) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = App::new(profiles, opts.insecure, tx);
    let host = opts
        .host
        .clone()
        .unwrap_or_else(|| app.profiles.default_name().to_owned());
    app.on_connect_result(opts.clone(), host, initial);

    let mut terminal = ratatui::init();
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let result = loop {
        if let Err(e) = terminal.draw(|f| ui::draw(f, &mut app)) {
            break Err(e.into());
        }
        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(ev)) => app.on_event(ev),
                Some(Err(e)) => break Err(e.into()),
                None => break Ok(()),
            },
            Some(msg) = rx.recv() => app.on_msg(msg),
            _ = tick.tick() => app.on_tick(),
        }
        if app.quit {
            break Ok(());
        }
    };
    ratatui::restore();
    result
}
