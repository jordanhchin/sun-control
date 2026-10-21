//! Applications screen.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};
use sun_control::api::types::{App as SunApp, truthy};

use super::app::{Action, App, Confirm, Load, Modal, Msg, ToastKind};
use super::app_editor::AppEditor;
use super::ui::{ACCENT, card, muted, not_connected, row};

pub struct AppsState {
    pub list: Load<Vec<SunApp>>,
    pub table: TableState,
}

impl Default for AppsState {
    fn default() -> Self {
        Self {
            list: Load::Idle,
            table: TableState::default().with_selected(Some(0)),
        }
    }
}

impl AppsState {
    pub fn apps(&self) -> &[SunApp] {
        match &self.list {
            Load::Ready(a) => a,
            _ => &[],
        }
    }

    fn selected(&self) -> Option<(usize, &SunApp)> {
        let i = self.table.selected()?;
        self.apps().get(i).map(|a| (i, a))
    }
}

impl App {
    pub(super) fn load_apps(&mut self) {
        let Some(client) = self.client().cloned() else {
            return;
        };
        if !matches!(self.apps.list, Load::Ready(_)) {
            self.apps.list = Load::Loading;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(Msg::Apps(client.apps().await.map(|l| l.apps)));
        });
    }

    fn platform(&self) -> Option<String> {
        match &self.info {
            Load::Ready(i) => i.platform.clone(),
            _ => None,
        }
    }

    pub(super) fn apps_key(&mut self, key: KeyEvent) {
        if self.client().is_none() {
            return;
        }
        let n = self.apps.apps().len();
        let sel = self.apps.table.selected().unwrap_or(0);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('J') => self.move_app(1),
            KeyCode::Char('K') => self.move_app(-1),
            KeyCode::Down if shift => self.move_app(1),
            KeyCode::Up if shift => self.move_app(-1),
            KeyCode::Down | KeyCode::Char('j') => self
                .apps
                .table
                .select(Some((sel + 1).min(n.saturating_sub(1)))),
            KeyCode::Up | KeyCode::Char('k') => self.apps.table.select(Some(sel.saturating_sub(1))),
            KeyCode::Home | KeyCode::Char('g') => self.apps.table.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') => self.apps.table.select(Some(n.saturating_sub(1))),
            KeyCode::Char('a') => {
                let editor = AppEditor::new(SunApp::default(), None, self.platform().as_deref());
                self.modal = Some(Modal::Editor(Box::new(editor)));
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                if let Some((i, app)) = self.apps.selected() {
                    let editor = AppEditor::new(app.clone(), Some(i), self.platform().as_deref());
                    self.modal = Some(Modal::Editor(Box::new(editor)));
                }
            }
            KeyCode::Char('c') => {
                if let Some((_, app)) = self.apps.selected() {
                    let mut copy = app.clone();
                    copy.name = format!("{} (copy)", app.name);
                    // A copy must not claim the original's identity.
                    copy.extra.remove("uuid");
                    let editor = AppEditor::new(copy, None, self.platform().as_deref());
                    self.modal = Some(Modal::Editor(Box::new(editor)));
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if let Some((index, app)) = self.apps.selected() {
                    self.modal = Some(Modal::Confirm(Confirm {
                        title: format!("Delete “{}”?", app.name),
                        body: "It will no longer be shown in Moonlight. This can't be undone."
                            .into(),
                        action: Action::DeleteApp {
                            index,
                            name: app.name.clone(),
                        },
                    }));
                }
            }
            _ => {}
        }
    }

    /// Sunshine has no reorder endpoint, so moving swaps two entries in place.
    fn move_app(&mut self, delta: isize) {
        let Some((from, a)) = self.apps.selected().map(|(i, a)| (i, a.clone())) else {
            return;
        };
        let to = from as isize + delta;
        let Some(b) = usize::try_from(to)
            .ok()
            .and_then(|t| self.apps.apps().get(t))
            .cloned()
        else {
            return;
        };
        let to = to as usize;
        if let Load::Ready(list) = &mut self.apps.list {
            list.swap(from, to);
        }
        self.apps.table.select(Some(to));
        self.run_api(move |c| async move {
            c.save_app(&a, Some(to)).await?;
            c.save_app(&b, Some(from)).await?;
            Ok(String::new())
        });
    }

    pub(super) fn save_editor(&mut self, editor: Box<AppEditor>) {
        let app = match editor.to_app() {
            Ok(app) => app,
            Err(e) => {
                let mut editor = editor;
                editor.error = Some(e);
                self.modal = Some(Modal::Editor(editor));
                return;
            }
        };
        let index = editor.index;
        let name = app.name.clone();
        if index.is_none() {
            // Select the new app once the list reloads.
            self.apps.table.select(Some(self.apps.apps().len()));
        }
        self.toast(format!("Saving “{name}”…"), ToastKind::Info);
        self.run_api(move |c| async move {
            c.save_app(&app, index).await?;
            Ok(format!("Saved “{name}”"))
        });
    }
}

fn summary(app: &SunApp) -> String {
    if let Some(cmd) = app.cmd.as_deref().filter(|c| !c.is_empty()) {
        cmd.to_owned()
    } else if let Some(d) = app.detached.first() {
        format!("{d} (detached)")
    } else {
        "desktop".to_owned()
    }
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if app.client().is_none() {
        return not_connected(f, app, area);
    }
    let area = area.inner(Margin::new(1, 0));
    let apps = match &app.apps.list {
        Load::Ready(list) => list,
        Load::Failed(e) => {
            let p = Paragraph::new(vec![
                Line::styled(
                    "Couldn't load applications",
                    Style::new().fg(super::ui::BAD).bold(),
                ),
                Line::styled(e.clone(), muted()),
            ]);
            f.render_widget(p.block(card("Applications")), area);
            return;
        }
        _ => {
            f.render_widget(
                Paragraph::new(Line::styled("Loading…", muted())).block(card("Applications")),
                area,
            );
            return;
        }
    };

    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area);
    let rows: Vec<Row> = apps
        .iter()
        .enumerate()
        .map(|(i, a)| {
            Row::new(vec![
                Cell::from(Span::styled(format!("{:>2}", i + 1), muted())),
                Cell::from(a.name.clone()).bold(),
                Cell::from(Span::styled(summary(a), muted())),
            ])
        })
        .collect();
    let title = format!("Applications ({})", apps.len());
    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Percentage(45),
            Constraint::Fill(1),
        ],
    )
    .block(card(&title))
    .row_highlight_style(Style::new().bg(Color::Indexed(236)).fg(ACCENT))
    .highlight_symbol("› ");
    let empty = apps.is_empty();
    f.render_stateful_widget(table, list_area, &mut app.apps.table);
    if empty {
        let inner = list_area.inner(Margin::new(2, 1));
        f.render_widget(
            Line::styled("No applications yet. Press a to add one.", muted()),
            inner,
        );
    }

    let Some((_, sel)) = app.apps.selected() else {
        f.render_widget(card("Details"), detail_area);
        return;
    };
    let flag = |key: &str, default: bool| sel.extra.get(key).map_or(default, truthy);
    let mut lines = vec![
        row("Name", Span::styled(sel.name.clone(), Style::new().bold())),
        row(
            "Command",
            sel.cmd
                .clone()
                .filter(|c| !c.is_empty())
                .unwrap_or("—".into()),
        ),
        row("Image", sel.image_path.clone().unwrap_or("default".into())),
    ];
    if let Some(serde_json::Value::String(o)) = sel.extra.get("output") {
        lines.push(row("Output log", o.clone()));
    }
    let mut badges = Vec::new();
    if flag("auto-detach", true) {
        badges.push("auto-detach");
    }
    if flag("wait-all", true) {
        badges.push("wait for all");
    }
    if flag("exclude-global-prep-cmd", false) {
        badges.push("no global prep");
    }
    if flag("elevated", false) {
        badges.push("elevated");
    }
    lines.push(row("Behaviour", badges.join(" · ")));
    if !sel.detached.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Detached commands",
            Style::new().fg(ACCENT).bold(),
        ));
        lines.extend(sel.detached.iter().map(|d| Line::raw(format!("  {d}"))));
    }
    if !sel.prep_cmd.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Prep commands",
            Style::new().fg(ACCENT).bold(),
        ));
        for p in &sel.prep_cmd {
            let show = |s: &str| {
                if s.is_empty() {
                    "—".to_owned()
                } else {
                    s.to_owned()
                }
            };
            lines.push(Line::from(vec![
                Span::styled("  do    ", muted()),
                Span::raw(show(&p.r#do)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  undo  ", muted()),
                Span::raw(show(&p.undo)),
            ]));
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(card("Details"))
            .wrap(Wrap { trim: false }),
        detail_area,
    );
}
