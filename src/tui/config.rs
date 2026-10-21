//! Configuration screen.

use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use sun_control::api::ApiError;
use sun_control::api::types::Config;
use sun_control::schema::{self, Choice, Field, Kind, Schema, Tab};

use super::app::{Action, App, Confirm, FormKind, Load, Modal, Msg, ToastKind};
use super::ui::{ACCENT, BAD, card, hints, modal_block, muted, not_connected, pane};
use super::widgets::{Field as FormField, Form, Input, centered};

#[derive(Default)]
pub struct ConfigState {
    pub load: Load<()>,
    pub tabs: Vec<Tab>,
    pub version: Option<String>,
    /// Values as saved on the server, as text.
    pub saved: BTreeMap<String, String>,
    /// The same values exactly as the server sent them.
    pub original: serde_json::Map<String, serde_json::Value>,
    /// Pending changes; `None` resets the key to its default.
    pub edits: BTreeMap<String, Option<String>>,
    pub tab: usize,
    pub field: usize,
    scroll: usize,
    pub search: Option<String>,
}

pub struct Picker {
    pub key: String,
    pub title: String,
    pub options: Vec<Choice>,
    pub selected: usize,
}

impl ConfigState {
    fn set_config(&mut self, schema: &Schema, cfg: Config) {
        self.tabs = schema.resolve(cfg.platform.as_deref(), &cfg.values);
        self.saved = cfg
            .values
            .iter()
            .map(|(k, v)| (k.clone(), schema::as_text(v)))
            .collect();
        self.original = cfg.values.clone();
        self.version = cfg.version;
        self.load = Load::Ready(());
        self.tab = self.tab.min(self.tabs.len().saturating_sub(1));
    }

    pub fn dirty(&self) -> usize {
        self.edits.len()
    }

    /// The value that would be saved: pending edit, else server value.
    fn current(&self, key: &str) -> Option<String> {
        match self.edits.get(key) {
            Some(edit) => edit.clone(),
            None => self.saved.get(key).cloned(),
        }
    }

    fn effective(&self, field: &Field) -> String {
        self.current(&field.key)
            .unwrap_or_else(|| field.default_text())
    }

    fn set(&mut self, field: &Field, value: Option<String>) {
        // A value equal to the default is stored as "unset", like the web UI.
        let value = value.filter(|v| !field.is_default(v));
        let saved = self
            .saved
            .get(&field.key)
            .filter(|v| !field.is_default(v))
            .cloned();
        if value == saved {
            self.edits.remove(&field.key);
        } else {
            self.edits.insert(field.key.clone(), value);
        }
    }

    /// Fields on screen: the current tab, or every match for a search.
    fn visible(&self) -> Vec<(usize, &Field)> {
        match &self.search {
            Some(q) => {
                let q = q.to_lowercase();
                self.tabs
                    .iter()
                    .enumerate()
                    .flat_map(|(ti, t)| t.fields.iter().map(move |f| (ti, f)))
                    .filter(|(_, f)| {
                        f.key.to_lowercase().contains(&q)
                            || f.label.to_lowercase().contains(&q)
                            || f.desc
                                .as_deref()
                                .is_some_and(|d| d.to_lowercase().contains(&q))
                    })
                    .collect()
            }
            None => self.tabs.get(self.tab).map_or_else(Vec::new, |t| {
                t.fields.iter().map(|f| (self.tab, f)).collect()
            }),
        }
    }

    fn focused(&self) -> Option<Field> {
        self.visible().get(self.field).map(|(_, f)| (*f).clone())
    }

    fn field_by_key(&self, key: &str) -> Option<Field> {
        self.tabs
            .iter()
            .flat_map(|t| &t.fields)
            .find(|f| f.key == key)
            .cloned()
    }

    /// Everything that would be stored after saving.
    fn merged(&self) -> BTreeMap<String, String> {
        let mut values = self.saved.clone();
        for (k, v) in &self.edits {
            match v {
                Some(v) => values.insert(k.clone(), v.clone()),
                None => values.remove(k),
            };
        }
        values
    }

    fn diff_text(&self) -> String {
        self.edits
            .iter()
            .map(|(key, new)| {
                let field = self.field_by_key(key);
                let label = field.as_ref().map_or(key.as_str(), |f| f.label.as_str());
                let show = |v: Option<&String>| match (v, &field) {
                    (Some(v), Some(f)) => f.display(v),
                    (Some(v), None) => v.clone(),
                    (None, Some(f)) => {
                        format!("{} (default)", nonempty(&f.display(&f.default_text())))
                    }
                    (None, None) => "(removed)".into(),
                };
                format!(
                    "• {label}: {} → {}",
                    show(self.saved.get(key)),
                    show(new.as_ref())
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl App {
    pub(super) fn load_config(&mut self) {
        let Some(client) = self.client().cloned() else {
            return;
        };
        if !matches!(self.config.load, Load::Ready(_)) {
            self.config.load = Load::Loading;
        }
        self.spawn(async move { Msg::Config(client.config().await) });
    }

    pub(super) fn on_config(&mut self, result: Result<Config, ApiError>) {
        match result {
            Ok(cfg) => self.config.set_config(&self.schema, cfg),
            Err(e) => self.config.load = Load::Failed(e.to_string()),
        }
    }

    pub(super) fn on_config_saved(&mut self, result: Result<(), ApiError>) {
        match result {
            Ok(()) => {
                self.config.edits.clear();
                self.load_config();
                self.modal = Some(Modal::Confirm(Confirm {
                    title: "Saved. Restart Sunshine now?".into(),
                    body: "Most settings only take effect after a restart. Active streams will be interrupted.".into(),
                    action: Action::Restart,
                }));
            }
            Err(e) => self.toast(format!("Couldn't save: {e}"), ToastKind::Error),
        }
    }

    pub(super) fn save_config(&mut self) {
        let body = match schema::to_save_body(
            &self.config.tabs,
            &self.config.merged(),
            &self.config.original,
        ) {
            Ok(b) => b,
            Err(e) => return self.toast(e, ToastKind::Error),
        };
        let Some(client) = self.client().cloned() else {
            return;
        };
        self.spawn(async move { Msg::ConfigSaved(client.save_config(&body).await) });
    }

    pub(super) fn set_config_value(&mut self, key: &str, value: String) -> Result<(), String> {
        let field = self.config.field_by_key(key).ok_or("unknown setting")?;
        field.validate(&value)?;
        self.config.set(&field, Some(value));
        Ok(())
    }

    pub(super) fn config_key(&mut self, key: KeyEvent) {
        if self.client().is_none() || !matches!(self.config.load, Load::Ready(_)) {
            return;
        }
        let n = self.config.visible().len();
        // Pane 1 lists sections; pane 2 (or any search) lists settings.
        let in_sections = self.focus == 1 && self.config.search.is_none();
        match key.code {
            KeyCode::Down | KeyCode::Char('j') if in_sections => self.next_section(1),
            KeyCode::Up | KeyCode::Char('k') if in_sections => {
                self.next_section(self.config.tabs.len().saturating_sub(1))
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ')
                if in_sections =>
            {
                self.focus = 2
            }
            KeyCode::Left | KeyCode::Char('h') if self.focus == 2 => self.focus = 1,
            KeyCode::Char(']') if self.config.search.is_none() => self.next_section(1),
            KeyCode::Char('[') if self.config.search.is_none() => {
                self.next_section(self.config.tabs.len().saturating_sub(1))
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.config.field = (self.config.field + 1).min(n.saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.config.field = self.config.field.saturating_sub(1)
            }
            KeyCode::PageDown => {
                self.config.field = (self.config.field + 10).min(n.saturating_sub(1))
            }
            KeyCode::PageUp => self.config.field = self.config.field.saturating_sub(10),
            KeyCode::Char('/') => {
                let form = Form::new(
                    "Search settings",
                    FormKind::ConfigSearch,
                    vec![FormField::new(
                        "Find",
                        Input::new(self.config.search.clone().unwrap_or_default()),
                    )],
                );
                self.modal = Some(Modal::Form(form));
            }
            KeyCode::Esc if self.config.search.is_some() => {
                self.config.search = None;
                self.config.field = 0;
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.edit_focused(),
            KeyCode::Char('x') | KeyCode::Delete => {
                if let Some(f) = self.config.focused() {
                    self.config.set(&f, None);
                }
            }
            KeyCode::Char('u') => {
                if let Some(f) = self.config.focused() {
                    self.config.edits.remove(&f.key);
                }
            }
            KeyCode::Char('U') if self.config.dirty() > 0 => {
                self.config.edits.clear();
                self.toast("Discarded all unsaved changes", ToastKind::Info);
            }
            // Plain `s` and Ctrl-S both save.
            KeyCode::Char('s') => {
                if self.config.dirty() == 0 {
                    return self.toast("No changes to save", ToastKind::Info);
                }
                if let Err(e) = schema::to_save_body(
                    &self.config.tabs,
                    &self.config.merged(),
                    &self.config.original,
                ) {
                    return self.toast(e, ToastKind::Error);
                }
                self.modal = Some(Modal::Confirm(Confirm {
                    title: format!(
                        "Save {} change{}?",
                        self.config.dirty(),
                        if self.config.dirty() == 1 { "" } else { "s" }
                    ),
                    body: self.config.diff_text(),
                    action: Action::SaveConfig,
                }));
            }
            _ => {}
        }
    }

    fn next_section(&mut self, by: usize) {
        let len = self.config.tabs.len().max(1);
        self.config.tab = (self.config.tab + by) % len;
        self.config.field = 0;
    }

    fn edit_focused(&mut self) {
        let Some(field) = self.config.focused() else {
            return;
        };
        let current = self.config.effective(&field);
        match field.kind {
            Kind::Toggle => {
                let next = if current == "enabled" {
                    "disabled"
                } else {
                    "enabled"
                };
                self.config.set(&field, Some(next.into()));
            }
            Kind::Enum => {
                let number = current.parse::<f64>().ok();
                let selected = field
                    .options
                    .iter()
                    .position(|o| {
                        o.value == current
                            || (number.is_some() && o.value.parse::<f64>().ok() == number)
                    })
                    .unwrap_or(0);
                self.modal = Some(Modal::Picker(Picker {
                    key: field.key.clone(),
                    title: field.label.clone(),
                    options: field.options.clone(),
                    selected,
                }));
            }
            Kind::Number | Kind::Text | Kind::Json => {
                let hint = match field.kind {
                    Kind::Number => "a number",
                    Kind::Json => "JSON",
                    _ => "",
                };
                let mut form = Form::new(
                    field.label.clone(),
                    FormKind::ConfigValue {
                        key: field.key.clone(),
                    },
                    vec![FormField::new("Value", Input::new(current)).hint(hint)],
                );
                form.focus = 0;
                let note = format!(
                    "{}  ·  default: {}",
                    field.key,
                    nonempty(&field.default_text())
                );
                self.modal = Some(Modal::Form(form.note(note)));
            }
        }
    }

    pub(super) fn picker_key(&mut self, mut picker: Picker, key: KeyEvent) {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                picker.selected = (picker.selected + 1).min(picker.options.len().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(choice) = picker.options.get(picker.selected) {
                    let _ = self.set_config_value(&picker.key, choice.value.clone());
                }
                return;
            }
            KeyCode::Esc | KeyCode::Char('q') => return,
            _ => {}
        }
        self.modal = Some(Modal::Picker(picker));
    }
}

fn nonempty(s: &str) -> String {
    if s.is_empty() {
        "(empty)".into()
    } else {
        s.to_owned()
    }
}

pub fn draw_picker(f: &mut Frame, picker: &Picker) {
    let height = picker.options.len() as u16 + 4;
    let width = picker
        .options
        .iter()
        .map(|o| o.label.chars().count())
        .max()
        .unwrap_or(10)
        .max(picker.title.chars().count()) as u16
        + 12;
    let area = centered(f.area(), width.min(70), height);
    f.render_widget(Clear, area);
    let lines: Vec<Line> = picker
        .options
        .iter()
        .enumerate()
        .map(|(i, o)| {
            if i == picker.selected {
                Line::from(vec![
                    Span::styled("› ", Style::new().fg(ACCENT)),
                    Span::styled(o.label.clone(), Style::new().fg(ACCENT).bold()),
                ])
            } else {
                Line::raw(format!("  {}", o.label))
            }
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(modal_block(&picker.title)),
        area,
    );
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if app.client().is_none() {
        return not_connected(f, app, area);
    }
    let area = area.inner(Margin::new(1, 0));
    let focus = app.focus;
    let state = &mut app.config;
    match &state.load {
        Load::Ready(()) => {}
        Load::Failed(e) => {
            let p = Paragraph::new(vec![
                Line::styled(
                    "Couldn't load the configuration",
                    Style::new().fg(BAD).bold(),
                ),
                Line::styled(e.clone(), muted()),
            ]);
            return f.render_widget(p.block(card("Configuration")), area);
        }
        _ => {
            return f.render_widget(
                Paragraph::new(Line::styled("Loading…", muted())).block(card("Configuration")),
                area,
            );
        }
    }

    let [tabs_area, main] =
        Layout::horizontal([Constraint::Length(28), Constraint::Min(0)]).areas(area);

    // Tabs, marking those with pending edits.
    let tab_lines: Vec<Line> = state
        .tabs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let active = state.search.is_none() && i == state.tab;
            let focused = active && focus == 1;
            let edited = t.fields.iter().any(|f| state.edits.contains_key(&f.key));
            let style = if focused {
                Style::new().fg(Color::Black).bg(ACCENT).bold()
            } else if active {
                Style::new().fg(ACCENT).bold()
            } else {
                Style::new()
            };
            Line::from(vec![
                Span::styled(if active { "› " } else { "  " }, Style::new().fg(ACCENT)),
                Span::styled(t.name.clone(), style),
                Span::styled(if edited { " ●" } else { "" }, Style::new().fg(ACCENT)),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(tab_lines).block(pane("Sections", focus == 1 && state.search.is_none())),
        tabs_area,
    );

    let [list_area, desc_area] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(6)]).areas(main);
    let count = state.visible().len();
    let title = match &state.search {
        Some(q) => format!(
            "Search “{q}” · {count} match{}",
            if count == 1 { "" } else { "es" }
        ),
        None => state
            .tabs
            .get(state.tab)
            .map(|t| t.name.clone())
            .unwrap_or_default(),
    };
    let mut block = pane(&title, focus == 2 || (focus == 1 && state.search.is_some()));
    if state.dirty() > 0 {
        let mut footer = hints(&[("s", "save"), ("U", "discard all")]);
        footer.spans.insert(
            0,
            Span::styled(
                format!(" {} unsaved ", state.dirty()),
                Style::new().fg(Color::Black).bg(ACCENT),
            ),
        );
        block = block.title_bottom(footer.right_aligned());
    }
    let inner = block.inner(list_area);
    f.render_widget(block, list_area);

    let height = inner.height as usize;
    let field_idx = state.field.min(count.saturating_sub(1));
    // Keep the focused setting on screen.
    if field_idx < state.scroll {
        state.scroll = field_idx;
    } else if field_idx >= state.scroll + height {
        state.scroll = field_idx + 1 - height;
    }
    let state = &*state;
    let visible = state.visible();
    let label_w = (inner.width as usize / 2).clamp(20, 40);
    let lines: Vec<Line> = visible
        .iter()
        .enumerate()
        .skip(state.scroll)
        .take(height)
        .map(|(i, (ti, field))| {
            let focused = i == field_idx;
            let value = state.effective(field);
            let edited = state.edits.contains_key(&field.key);
            let default = state
                .current(&field.key)
                .is_none_or(|v| field.is_default(&v));
            let shown = field.display(&value);
            let shown = if shown.is_empty() {
                "—".to_owned()
            } else {
                shown
            };
            let mut label = field.label.clone();
            if state.search.is_some() {
                label = format!("{} › {label}", state.tabs[*ti].name);
            }
            let label: String = if label.chars().count() > label_w {
                label.chars().take(label_w - 1).collect::<String>() + "…"
            } else {
                label
            };
            let value_style = if edited {
                Style::new().fg(ACCENT).bold()
            } else if default {
                Style::new()
            } else {
                Style::new().fg(Color::White).bold()
            };
            let mut spans = vec![
                Span::styled(if focused { "› " } else { "  " }, Style::new().fg(ACCENT)),
                Span::styled(
                    format!("{label:<label_w$}  "),
                    if focused {
                        Style::new().fg(ACCENT)
                    } else {
                        Style::new()
                    },
                ),
                Span::styled(shown, value_style),
            ];
            if edited {
                spans.push(Span::styled("  ●", Style::new().fg(ACCENT)));
            } else if default {
                spans.push(Span::styled("  default", muted()));
            }
            let line = Line::from(spans);
            if focused {
                line.style(Style::new().bg(Color::Indexed(236)))
            } else {
                line
            }
        })
        .collect();
    if lines.is_empty() {
        f.render_widget(Line::styled("No settings match.", muted()), inner);
    } else {
        f.render_widget(Paragraph::new(lines), inner);
    }

    // Description of the focused setting.
    let desc = match visible.get(field_idx) {
        Some((_, field)) => {
            let mut lines = vec![Line::from(vec![
                Span::styled(field.key.clone(), muted()),
                Span::styled(
                    format!(
                        "  ·  default: {}",
                        nonempty(&field.display(&field.default_text()))
                    ),
                    muted(),
                ),
            ])];
            if let Some(d) = &field.desc {
                lines.push(Line::raw(d.clone()));
            }
            lines
        }
        None => vec![],
    };
    f.render_widget(
        Paragraph::new(desc)
            .block(card("About this setting"))
            .wrap(Wrap { trim: true }),
        desc_area,
    );
}
