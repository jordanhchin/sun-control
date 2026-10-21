//! Full-screen editor for one application entry.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use serde_json::{Value, json};
use sun_control::api::types::{App, PrepCmd, truthy};

use super::ui::{ACCENT, hints, modal_block, muted};
use super::widgets::Input;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    ExcludeGlobalPrep,
    Elevated,
    AutoDetach,
    WaitAll,
}

impl Flag {
    fn key(self) -> &'static str {
        match self {
            Flag::ExcludeGlobalPrep => "exclude-global-prep-cmd",
            Flag::Elevated => "elevated",
            Flag::AutoDetach => "auto-detach",
            Flag::WaitAll => "wait-all",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Flag::ExcludeGlobalPrep => "Skip global prep commands",
            Flag::Elevated => "Run as administrator",
            Flag::AutoDetach => "Auto-detach launchers",
            Flag::WaitAll => "Wait for all processes",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Flag::ExcludeGlobalPrep => {
                "Don't run the global prep commands from the configuration for this app."
            }
            Flag::Elevated => "Launch the app with administrator privileges (Windows only).",
            Flag::AutoDetach => {
                "Keep streaming if the app exits quickly, for launchers that start another program and close."
            }
            Flag::WaitAll => "Keep streaming until every process started by the app has exited.",
        }
    }

    fn default(self) -> bool {
        matches!(self, Flag::AutoDetach | Flag::WaitAll)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Name,
    Command,
    Output,
    Image,
    Flag(Flag),
    ExitTimeout,
    Detached(usize),
    AddDetached,
    PrepDo(usize),
    PrepUndo(usize),
    AddPrep,
}

impl Row {
    fn section(self) -> &'static str {
        match self {
            Row::Name | Row::Command | Row::Output | Row::Image => "General",
            Row::Flag(_) | Row::ExitTimeout => "Behaviour",
            Row::Detached(_) | Row::AddDetached => "Detached commands",
            Row::PrepDo(_) | Row::PrepUndo(_) | Row::AddPrep => "Prep commands",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Row::Name => "The name shown in Moonlight.",
            Row::Command => "The main command to run. Leave empty for desktop-style entries.",
            Row::Output => "File to write the command's output to. Leave empty to discard it.",
            Row::Image => {
                "PNG cover image path sent to clients. Leave empty for the default box art."
            }
            Row::Flag(f) => f.help(),
            Row::ExitTimeout => {
                "Seconds to wait for the app to exit gracefully when quitting (0 = terminate immediately)."
            }
            Row::Detached(_) | Row::AddDetached => {
                "Commands started in the background and left running, e.g. `setsid steam steam://open/bigpicture`."
            }
            Row::PrepDo(_) | Row::PrepUndo(_) | Row::AddPrep => {
                "Run before the app starts (do) and after it ends (undo), e.g. to change resolution."
            }
        }
    }
}

pub enum EditorEvent {
    None,
    Save,
    Cancel,
}

pub struct AppEditor {
    /// `None` for a new app.
    pub index: Option<usize>,
    original: App,
    name: Input,
    command: Input,
    output: Input,
    image: Input,
    flags: Vec<(Flag, bool)>,
    exit_timeout: Input,
    detached: Vec<Input>,
    prep: Vec<(Input, Input, PrepCmd)>,
    focus: usize,
    scroll: usize,
    dirty: bool,
    /// Set after the first Esc on a dirty editor.
    confirm_discard: bool,
    pub error: Option<String>,
}

fn extra_str(app: &App, key: &str) -> String {
    match app.extra.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(v) => v.to_string(),
    }
}

impl AppEditor {
    pub fn new(app: App, index: Option<usize>, platform: Option<&str>) -> Self {
        let mut flags = vec![Flag::ExcludeGlobalPrep, Flag::AutoDetach, Flag::WaitAll];
        if platform == Some("windows") || app.extra.contains_key(Flag::Elevated.key()) {
            flags.insert(1, Flag::Elevated);
        }
        let flags = flags
            .into_iter()
            .map(|f| (f, app.extra.get(f.key()).map_or(f.default(), truthy)))
            .collect();
        let timeout = match app.extra.get("exit-timeout") {
            None | Some(Value::Null) => "5".to_owned(),
            Some(_) => extra_str(&app, "exit-timeout"),
        };
        Self {
            index,
            name: Input::new(app.name.clone()),
            command: Input::new(app.cmd.clone().unwrap_or_default()),
            output: Input::new(extra_str(&app, "output")),
            image: Input::new(app.image_path.clone().unwrap_or_default()),
            flags,
            exit_timeout: Input::new(timeout),
            detached: app.detached.iter().map(|d| Input::new(d.clone())).collect(),
            prep: app
                .prep_cmd
                .iter()
                .map(|p| {
                    (
                        Input::new(p.r#do.clone()),
                        Input::new(p.undo.clone()),
                        p.clone(),
                    )
                })
                .collect(),
            original: app,
            focus: 0,
            scroll: 0,
            dirty: false,
            confirm_discard: false,
            error: None,
        }
    }

    pub fn title(&self) -> String {
        match self.index {
            None => "New application".to_owned(),
            Some(_) => format!("Edit “{}”", self.original.name),
        }
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Name, Row::Command, Row::Output, Row::Image];
        rows.extend(self.flags.iter().map(|(f, _)| Row::Flag(*f)));
        rows.push(Row::ExitTimeout);
        rows.extend((0..self.detached.len()).map(Row::Detached));
        rows.push(Row::AddDetached);
        for i in 0..self.prep.len() {
            rows.push(Row::PrepDo(i));
            rows.push(Row::PrepUndo(i));
        }
        rows.push(Row::AddPrep);
        rows
    }

    fn input_mut(&mut self, row: Row) -> Option<&mut Input> {
        Some(match row {
            Row::Name => &mut self.name,
            Row::Command => &mut self.command,
            Row::Output => &mut self.output,
            Row::Image => &mut self.image,
            Row::ExitTimeout => &mut self.exit_timeout,
            Row::Detached(i) => &mut self.detached[i],
            Row::PrepDo(i) => &mut self.prep[i].0,
            Row::PrepUndo(i) => &mut self.prep[i].1,
            Row::Flag(_) | Row::AddDetached | Row::AddPrep => return None,
        })
    }

    fn input(&self, row: Row) -> Option<&Input> {
        Some(match row {
            Row::Name => &self.name,
            Row::Command => &self.command,
            Row::Output => &self.output,
            Row::Image => &self.image,
            Row::ExitTimeout => &self.exit_timeout,
            Row::Detached(i) => &self.detached[i],
            Row::PrepDo(i) => &self.prep[i].0,
            Row::PrepUndo(i) => &self.prep[i].1,
            Row::Flag(_) | Row::AddDetached | Row::AddPrep => return None,
        })
    }

    fn focus_row(&mut self, row: Row) {
        if let Some(i) = self.rows().iter().position(|r| *r == row) {
            self.focus = i;
        }
    }

    pub fn handle(&mut self, key: KeyEvent) -> EditorEvent {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let rows = self.rows();
        let row = rows[self.focus.min(rows.len() - 1)];
        if key.code != KeyCode::Esc {
            self.confirm_discard = false;
        }
        match key.code {
            KeyCode::Esc => {
                if !self.dirty || self.confirm_discard {
                    return EditorEvent::Cancel;
                }
                self.confirm_discard = true;
            }
            KeyCode::Char('s') if ctrl => return EditorEvent::Save,
            KeyCode::Down | KeyCode::Tab => self.focus = (self.focus + 1).min(rows.len() - 1),
            KeyCode::Up | KeyCode::BackTab => self.focus = self.focus.saturating_sub(1),
            KeyCode::PageDown => self.focus = (self.focus + 8).min(rows.len() - 1),
            KeyCode::PageUp => self.focus = self.focus.saturating_sub(8),
            // Remove the focused list entry.
            KeyCode::Char('d') if ctrl => {
                match row {
                    Row::Detached(i) => {
                        self.detached.remove(i);
                    }
                    Row::PrepDo(i) | Row::PrepUndo(i) => {
                        self.prep.remove(i);
                    }
                    _ => return EditorEvent::None,
                }
                self.dirty = true;
                self.focus = self.focus.min(self.rows().len() - 1);
            }
            KeyCode::Enter | KeyCode::Char(' ') if matches!(row, Row::Flag(_)) => {
                if let Row::Flag(f) = row
                    && let Some(entry) = self.flags.iter_mut().find(|(g, _)| *g == f)
                {
                    entry.1 = !entry.1;
                    self.dirty = true;
                }
            }
            KeyCode::Enter if row == Row::AddDetached => {
                self.detached.push(Input::default());
                self.dirty = true;
                self.focus_row(Row::Detached(self.detached.len() - 1));
            }
            KeyCode::Enter if row == Row::AddPrep => {
                self.prep
                    .push((Input::default(), Input::default(), PrepCmd::default()));
                self.dirty = true;
                self.focus_row(Row::PrepDo(self.prep.len() - 1));
            }
            KeyCode::Enter => self.focus = (self.focus + 1).min(rows.len() - 1),
            _ => {
                if let Some(input) = self.input_mut(row) {
                    let before = input.value.clone();
                    input.handle(key);
                    self.dirty |= input.value != before;
                }
            }
        }
        EditorEvent::None
    }

    /// Build the app to save, keeping any fields this editor doesn't know about.
    pub fn to_app(&self) -> Result<App, String> {
        let name = self.name.value.trim();
        if name.is_empty() {
            return Err("The app needs a name.".into());
        }
        let timeout: u32 = self
            .exit_timeout
            .value
            .trim()
            .parse()
            .map_err(|_| "Exit timeout must be a whole number of seconds.".to_owned())?;
        let opt = |s: &str| Some(s.trim().to_owned()).filter(|s| !s.is_empty());

        let mut app = self.original.clone();
        app.name = name.to_owned();
        app.cmd = opt(&self.command.value);
        app.image_path = opt(&self.image.value);
        match opt(&self.output.value) {
            Some(o) => app.extra.insert("output".into(), json!(o)),
            None => app.extra.remove("output"),
        };
        for (flag, value) in &self.flags {
            app.extra.insert(flag.key().into(), json!(value));
        }
        app.extra.insert("exit-timeout".into(), json!(timeout));
        app.detached = self.detached.iter().filter_map(|d| opt(&d.value)).collect();
        app.prep_cmd = self
            .prep
            .iter()
            .filter(|(d, u, _)| !d.value.trim().is_empty() || !u.value.trim().is_empty())
            .map(|(d, u, orig)| PrepCmd {
                r#do: d.value.trim().to_owned(),
                undo: u.value.trim().to_owned(),
                ..orig.clone()
            })
            .collect();
        Ok(app)
    }

    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let area = super::widgets::centered(
            area,
            area.width.saturating_sub(8).min(100),
            area.height.saturating_sub(2),
        );
        f.render_widget(Clear, area);
        let title = self.title();
        let block = modal_block(&title);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [body, help, footer] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .areas(inner);

        let label_w = 28usize;
        let value_w = (body.width as usize).saturating_sub(label_w + 2);
        let rows = self.rows();
        let mut lines: Vec<Line> = Vec::new();
        let mut focus_line = 0;
        let mut section = "";
        for (i, row) in rows.iter().enumerate() {
            if row.section() != section {
                if !lines.is_empty() {
                    lines.push(Line::raw(""));
                }
                section = row.section();
                lines.push(Line::styled(section, Style::new().fg(ACCENT).bold()));
            }
            let focused = i == self.focus;
            if focused {
                focus_line = lines.len();
            }
            let label = match row {
                Row::Name => "Name".to_owned(),
                Row::Command => "Command".to_owned(),
                Row::Output => "Output log file".to_owned(),
                Row::Image => "Image".to_owned(),
                Row::Flag(f) => f.label().to_owned(),
                Row::ExitTimeout => "Exit timeout (s)".to_owned(),
                Row::Detached(n) => format!("Command {}", n + 1),
                Row::PrepDo(n) => format!("#{} do", n + 1),
                Row::PrepUndo(_) => "   undo".to_owned(),
                Row::AddDetached => "+ Add detached command".to_owned(),
                Row::AddPrep => "+ Add prep command".to_owned(),
            };
            let marker = Span::styled(if focused { "› " } else { "  " }, Style::new().fg(ACCENT));
            let label_style = if focused {
                Style::new().fg(ACCENT).bold()
            } else {
                Style::new()
            };
            let mut spans = vec![
                marker,
                Span::styled(format!("{label:<label_w$}"), label_style),
            ];
            match row {
                Row::Flag(f) => {
                    let on = self
                        .flags
                        .iter()
                        .find(|(g, _)| g == f)
                        .is_some_and(|(_, v)| *v);
                    spans.push(if on {
                        Span::styled("[✓] on", Style::new().fg(super::ui::OK))
                    } else {
                        Span::styled("[ ] off", muted())
                    });
                }
                Row::AddDetached | Row::AddPrep => {}
                _ => {
                    let input = self.input(*row).expect("text row");
                    if input.value.is_empty() && !focused {
                        spans.push(Span::styled("—", muted()));
                    } else {
                        spans.extend(input.line(value_w, focused).spans);
                    }
                }
            }
            lines.push(Line::from(spans));
        }

        // Keep the focused row on screen.
        let height = body.height as usize;
        if focus_line < self.scroll + 1 {
            self.scroll = focus_line.saturating_sub(1);
        } else if focus_line >= self.scroll + height {
            self.scroll = focus_line + 1 - height;
        }
        let visible: Vec<Line> = lines.into_iter().skip(self.scroll).take(height).collect();
        f.render_widget(Paragraph::new(visible), body);

        let row = rows[self.focus.min(rows.len() - 1)];
        let help_text = if let Some(e) = &self.error {
            Line::styled(e.clone(), Style::new().fg(super::ui::BAD))
        } else if self.confirm_discard {
            Line::styled(
                "You have unsaved changes. Press Esc again to discard them.",
                Style::new().fg(ACCENT),
            )
        } else {
            Line::styled(row.help(), muted())
        };
        f.render_widget(
            Paragraph::new(help_text).wrap(Wrap { trim: true }),
            pad_top(help),
        );

        let mut keys = vec![("Ctrl-S", "save"), ("Esc", "cancel"), ("↑↓", "move")];
        match row {
            Row::Flag(_) => keys.push(("Space", "toggle")),
            Row::Detached(_) | Row::PrepDo(_) | Row::PrepUndo(_) => keys.push(("Ctrl-D", "remove")),
            Row::AddDetached | Row::AddPrep => keys.push(("Enter", "add")),
            _ => {}
        }
        f.render_widget(
            hints(&keys).alignment(ratatui::layout::Alignment::Right),
            footer,
        );
    }
}

fn pad_top(area: Rect) -> Rect {
    Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_str(e: &mut AppEditor, s: &str) {
        for c in s.chars() {
            e.handle(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    fn press(e: &mut AppEditor, code: KeyCode) {
        e.handle(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn round_trip_keeps_unknown_fields_and_types() {
        let app: App = serde_json::from_value(json!({
            "name": "Steam", "auto-detach": "false", "future": 1,
            "prep-cmd": [{"do": "a", "undo": "b", "elevated": true}]
        }))
        .unwrap();
        let out = AppEditor::new(app, Some(0), Some("linux"))
            .to_app()
            .unwrap();
        let v = serde_json::to_value(out).unwrap();
        assert_eq!(v["future"], 1);
        assert_eq!(v["auto-detach"], false);
        assert_eq!(v["wait-all"], true);
        assert_eq!(v["exit-timeout"], 5);
        assert_eq!(v["prep-cmd"][0]["elevated"], true);
    }

    #[test]
    fn adds_and_removes_entries() {
        let mut e = AppEditor::new(App::default(), None, Some("linux"));
        assert!(e.to_app().is_err(), "name is required");
        type_str(&mut e, "Game");
        e.focus_row(Row::AddDetached);
        press(&mut e, KeyCode::Enter);
        type_str(&mut e, "setsid game");
        e.focus_row(Row::AddPrep);
        press(&mut e, KeyCode::Enter);
        type_str(&mut e, "xrandr");
        let app = e.to_app().unwrap();
        assert_eq!(app.detached, vec!["setsid game"]);
        assert_eq!(app.prep_cmd[0].r#do, "xrandr");

        e.focus_row(Row::Detached(0));
        e.handle(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(e.to_app().unwrap().detached.is_empty());
    }

    #[test]
    fn esc_with_changes_needs_confirmation() {
        let mut e = AppEditor::new(App::default(), None, None);
        type_str(&mut e, "x");
        assert!(matches!(
            e.handle(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            EditorEvent::None
        ));
        assert!(matches!(
            e.handle(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            EditorEvent::Cancel
        ));
    }
}
