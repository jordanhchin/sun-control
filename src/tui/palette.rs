//! Command palette: type to find any screen or action.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::app::{App, Modal, Screen};
use super::system::SysAction;
use super::ui::{ACCENT, modal_block, muted};
use super::widgets::{Input, centered};

#[derive(Debug, Clone)]
enum Command {
    Go(Screen),
    /// Run a screen's own key binding, so the palette can't drift from it.
    Key(Screen, usize, char),
    System(SysAction),
    Connect(String),
    Help,
    Quit,
}

struct Item {
    label: String,
    detail: &'static str,
    command: Command,
}

pub struct Palette {
    input: Input,
    selected: usize,
    items: Vec<Item>,
}

/// Fuzzy match: every query character must appear in order. Lower is better:
/// gaps between matched characters cost, runs and word starts are free, and
/// ties go to the match that starts earlier.
fn score(label: &str, query: &str) -> Option<usize> {
    let label = label.to_lowercase();
    let mut pos = 0;
    let mut gaps = 0;
    let mut first = None;
    for q in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = label[pos..].find(q)?;
        let at = pos + found;
        let word_start = at == 0 || label[..at].ends_with([' ', ':']);
        if found > 0 && !word_start {
            gaps += found;
        }
        first.get_or_insert(at);
        pos = at + q.len_utf8();
    }
    Some(gaps * 100 + first.unwrap_or(0))
}

impl Palette {
    pub fn new(app: &App) -> Self {
        let mut items: Vec<Item> = Screen::ALL
            .iter()
            .map(|s| Item {
                label: format!("Go to {}", s.title()),
                detail: "screen",
                command: Command::Go(*s),
            })
            .collect();
        let key = |label: &str, detail, screen, pane, c| Item {
            label: label.into(),
            detail,
            command: Command::Key(screen, pane, c),
        };
        if app.client().is_some() {
            items.extend([
                key("Add application", "apps", Screen::Apps, 1, 'a'),
                key("Pair a device", "pairing", Screen::Pairing, 1, 'p'),
                key("Search settings", "config", Screen::Config, 2, '/'),
                key(
                    "Review and save configuration",
                    "config",
                    Screen::Config,
                    2,
                    's',
                ),
                key("Search logs", "logs", Screen::Logs, 1, '/'),
            ]);
            for a in SysAction::for_platform(app.platform_name().as_deref()) {
                items.push(Item {
                    label: a.title().into(),
                    detail: "system",
                    command: Command::System(a),
                });
            }
        }
        items.push(key("Add host", "hosts", Screen::Hosts, 1, 'a'));
        for name in app.host_names() {
            items.push(Item {
                label: format!("Connect to {name}"),
                detail: "host",
                command: Command::Connect(name),
            });
        }
        items.push(Item {
            label: "Keyboard shortcuts".into(),
            detail: "help",
            command: Command::Help,
        });
        items.push(Item {
            label: "Quit".into(),
            detail: "",
            command: Command::Quit,
        });
        Self {
            input: Input::default(),
            selected: 0,
            items,
        }
    }

    fn matches(&self) -> Vec<&Item> {
        let query = self.input.value.trim();
        let mut scored: Vec<(usize, usize, &Item)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| score(&item.label, query).map(|s| (s, i, item)))
            .collect();
        scored.sort_by_key(|(s, i, _)| (*s, *i));
        scored.into_iter().map(|(_, _, item)| item).collect()
    }
}

impl App {
    pub(super) fn palette_key(&mut self, mut palette: Palette, key: KeyEvent) {
        let n = palette.matches().len();
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Down => palette.selected = (palette.selected + 1).min(n.saturating_sub(1)),
            KeyCode::Up => palette.selected = palette.selected.saturating_sub(1),
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                palette.selected = (palette.selected + 1).min(n.saturating_sub(1))
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                palette.selected = palette.selected.saturating_sub(1)
            }
            KeyCode::Enter => {
                if let Some(item) = palette.matches().get(palette.selected) {
                    let command = item.command.clone();
                    self.run_command(command);
                }
                return;
            }
            _ => {
                if palette.input.handle(key) {
                    palette.selected = 0;
                }
            }
        }
        self.modal = Some(Modal::Palette(Box::new(palette)));
    }

    fn run_command(&mut self, command: Command) {
        match command {
            Command::Go(screen) => {
                self.switch(screen);
                self.focus = 1;
            }
            Command::Key(screen, pane, c) => {
                self.switch(screen);
                self.focus = pane;
                self.on_event(crossterm::event::Event::Key(KeyEvent::new(
                    KeyCode::Char(c),
                    KeyModifiers::NONE,
                )));
            }
            Command::System(action) => self.start_system_action(action),
            Command::Connect(host) => self.connect_to(&host),
            Command::Help => self.modal = Some(Modal::Help),
            Command::Quit => self.on_event(crossterm::event::Event::Key(KeyEvent::new(
                KeyCode::Char('q'),
                KeyModifiers::NONE,
            ))),
        }
    }
}

pub fn draw(f: &mut Frame, palette: &Palette) {
    let matches = palette.matches();
    let rows = matches.len().clamp(1, 12) as u16;
    let area = centered(f.area(), 64, rows + 6);
    f.render_widget(Clear, area);
    let block = modal_block("Command palette");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [input_area, _, list_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(inner);

    let mut prompt = vec![Span::styled("› ", Style::new().fg(ACCENT))];
    prompt.extend(
        palette
            .input
            .line(input_area.width.saturating_sub(2) as usize, true)
            .spans,
    );
    f.render_widget(Line::from(prompt), input_area);

    let start = palette.selected.saturating_sub(rows as usize - 1);
    let width = list_area.width as usize;
    let lines: Vec<Line> = if matches.is_empty() {
        vec![Line::styled("No matching commands", muted())]
    } else {
        matches
            .iter()
            .enumerate()
            .skip(start)
            .take(rows as usize)
            .map(|(i, item)| {
                let pad = width.saturating_sub(item.label.chars().count() + item.detail.len() + 2);
                let line = Line::from(vec![
                    Span::raw(format!(" {}", item.label)),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(item.detail, muted()),
                ]);
                if i == palette.selected {
                    line.style(Style::new().fg(ACCENT).bg(Color::Indexed(236)).bold())
                } else {
                    line
                }
            })
            .collect()
    };
    f.render_widget(Paragraph::new(lines), list_area);
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn fuzzy_scoring() {
        assert!(score("Restart Sunshine", "rest").is_some());
        assert!(score("Restart Sunshine", "rs").is_some());
        assert!(score("Restart Sunshine", "xyz").is_none());
        // Word-start matches beat scattered ones.
        assert!(
            score("Go to Logs", "logs")
                < score("Close the running app", "logs").or(Some(usize::MAX))
        );
        assert!(score("Pair a device", "pair") < score("Go to Pairing", "pair"));
    }
}
