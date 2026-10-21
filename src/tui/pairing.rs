//! Pairing screen: devices waiting to pair and already-paired clients.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};
use sun_control::api::ApiError;
use sun_control::api::types::{Client, PendingPairing};
use sun_control::devices::format_date;

use super::app::{Action, App, Confirm, FormKind, Load, Modal, Msg};
use super::ui::{ACCENT, BAD, OK, card, hints, muted, not_connected};
use super::widgets::{Field, Form, Input};

pub const PAIRING_REFRESH: Duration = Duration::from_secs(3);

pub enum Pending {
    Loading,
    Ready(Vec<PendingPairing>),
    /// Older Sunshine: pairing is by PIN alone.
    Legacy,
    Failed(String),
}

pub struct PairingState {
    pub pending: Pending,
    pub clients: Load<Vec<Client>>,
    /// Selection across both lists: pending requests first, then clients.
    pub selected: usize,
    pub at: Option<Instant>,
    pub in_flight: bool,
}

impl Default for PairingState {
    fn default() -> Self {
        Self {
            pending: Pending::Loading,
            clients: Load::Loading,
            selected: 0,
            at: None,
            in_flight: false,
        }
    }
}

enum Selected<'a> {
    Pending(&'a PendingPairing),
    Client(&'a Client),
}

impl PairingState {
    fn pending(&self) -> &[PendingPairing] {
        match &self.pending {
            Pending::Ready(p) => p,
            _ => &[],
        }
    }

    fn clients(&self) -> &[Client] {
        match &self.clients {
            Load::Ready(c) => c,
            _ => &[],
        }
    }

    fn len(&self) -> usize {
        self.pending().len() + self.clients().len()
    }

    fn selection(&self) -> Option<Selected<'_>> {
        let p = self.pending();
        if self.selected < p.len() {
            Some(Selected::Pending(&p[self.selected]))
        } else {
            self.clients()
                .get(self.selected - p.len())
                .map(Selected::Client)
        }
    }

    pub fn set(
        &mut self,
        pending: Result<Vec<PendingPairing>, ApiError>,
        clients: Result<Vec<Client>, ApiError>,
    ) {
        self.in_flight = false;
        self.pending = match pending {
            Ok(p) => Pending::Ready(p),
            Err(ApiError::Unsupported(_)) => Pending::Legacy,
            Err(e) => Pending::Failed(e.to_string()),
        };
        self.clients = match clients {
            Ok(mut c) => {
                // Same-named devices end up adjacent, oldest first.
                c.sort_by_key(|c| (c.name.to_lowercase(), c.created()));
                Load::Ready(c)
            }
            Err(e) => Load::Failed(e.to_string()),
        };
        self.selected = self.selected.min(self.len().saturating_sub(1));
    }
}

impl App {
    pub(super) fn load_pairing(&mut self) {
        if self.pairing.in_flight {
            return;
        }
        let Some(client) = self.client().cloned() else {
            return;
        };
        self.pairing.in_flight = true;
        self.pairing.at = Some(Instant::now());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let (pending, clients) = tokio::join!(client.pending_pairings(), client.clients());
            let _ = tx.send(Msg::Pairing { pending, clients });
        });
    }

    fn open_pair_form(&mut self, request: Option<&PendingPairing>) {
        let (pairing_id, name, title) = match request {
            Some(p) => (
                Some(p.id.clone()),
                p.name.clone(),
                format!("Pair {}", p.label()),
            ),
            None => (None, String::new(), "Pair a device".to_owned()),
        };
        let form = Form::new(
            title,
            FormKind::Pair { pairing_id },
            vec![
                Field::new("PIN", Input::default()).hint("the 4 digits Moonlight shows"),
                Field::new("Device name", Input::new(name)).hint("how it will be listed here"),
            ],
        )
        .note("Only pair devices you control: a paired device can fully control this computer.");
        self.modal = Some(Modal::Form(form));
    }

    pub(super) fn submit_pair(&mut self, form: Form<FormKind>) {
        let FormKind::Pair { pairing_id } = form.kind.clone() else {
            return;
        };
        let pin = form.value(0).to_owned();
        if pin.len() != 4 || !pin.chars().all(|c| c.is_ascii_digit()) {
            self.toast("The PIN is 4 digits", super::app::ToastKind::Error);
            self.modal = Some(Modal::Form(form));
            return;
        }
        let name = Some(form.value(1).to_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or("Moonlight".into());
        self.run_api(move |c| async move {
            c.pair(&pin, &name, pairing_id.as_deref()).await?;
            Ok(format!("Paired “{name}”. Check Moonlight to continue."))
        });
    }

    pub(super) fn pairing_key(&mut self, key: KeyEvent) {
        if self.client().is_none() {
            return;
        }
        let n = self.pairing.len();
        let sel = self.pairing.selected;
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.pairing.selected = (sel + 1).min(n.saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => self.pairing.selected = sel.saturating_sub(1),
            KeyCode::Char('p') => {
                let request = match (&self.pairing.pending, self.pairing.selection()) {
                    (_, Some(Selected::Pending(p))) => Some(p.clone()),
                    (Pending::Ready(list), _) if list.len() == 1 => Some(list[0].clone()),
                    (Pending::Ready(list), _) if list.is_empty() => {
                        self.toast(
                            "No devices are waiting. Start pairing in Moonlight first.",
                            super::app::ToastKind::Error,
                        );
                        return;
                    }
                    (Pending::Ready(_), _) => {
                        self.toast(
                            "Select the device to pair first",
                            super::app::ToastKind::Error,
                        );
                        return;
                    }
                    _ => None,
                };
                self.open_pair_form(request.as_ref());
            }
            KeyCode::Enter => match self.pairing.selection() {
                Some(Selected::Pending(p)) => {
                    let p = p.clone();
                    self.open_pair_form(Some(&p));
                }
                Some(Selected::Client(c)) => self.toggle_client(c.clone()),
                None if matches!(self.pairing.pending, Pending::Legacy) => {
                    self.open_pair_form(None)
                }
                None => {}
            },
            KeyCode::Char(' ') | KeyCode::Char('e') => {
                if let Some(Selected::Client(c)) = self.pairing.selection() {
                    self.toggle_client(c.clone());
                }
            }
            KeyCode::Char('c') | KeyCode::Char('d') | KeyCode::Delete => {
                let action = match self.pairing.selection() {
                    Some(Selected::Pending(p)) => Confirm {
                        title: "Cancel pairing request?".into(),
                        body: format!("{} will have to start pairing again.", p.label()),
                        action: Action::CancelPairing { id: p.id.clone(), label: p.label() },
                    },
                    Some(Selected::Client(c)) => Confirm {
                        title: format!("Unpair “{}”?", display_name(c)),
                        body: "It will have to pair again to stream. If it's streaming now, the session continues until it ends or you close the running app.".into(),
                        action: Action::Unpair { uuid: c.uuid.clone(), name: display_name(c) },
                    },
                    None => return,
                };
                self.modal = Some(Modal::Confirm(action));
            }
            KeyCode::Char('D') if !self.pairing.clients().is_empty() => {
                self.modal = Some(Modal::Confirm(Confirm {
                    title: "Unpair all devices?".into(),
                    body: format!(
                        "All {} paired devices will have to pair again.",
                        self.pairing.clients().len()
                    ),
                    action: Action::UnpairAll,
                }));
            }
            _ => {}
        }
    }

    fn toggle_client(&mut self, client: Client) {
        let Some(enabled) = client.enabled() else {
            self.toast(
                "This Sunshine version can't disable clients",
                super::app::ToastKind::Error,
            );
            return;
        };
        let name = display_name(&client);
        self.run_api(move |c| async move {
            c.set_client_enabled(&client.uuid, !enabled).await?;
            Ok(format!(
                "{} “{name}”",
                if enabled { "Disabled" } else { "Enabled" }
            ))
        });
    }
}

fn display_name(c: &Client) -> String {
    if c.name.is_empty() {
        "Unknown client".into()
    } else {
        c.name.clone()
    }
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if app.client().is_none() {
        return not_connected(f, app, area);
    }
    let area = area.inner(Margin::new(1, 0));
    let state = &app.pairing;
    let pending_rows = state.pending().len().max(1) as u16;
    let [pending_area, clients_area] = Layout::vertical([
        Constraint::Length(pending_rows.max(2) + 2),
        Constraint::Min(5),
    ])
    .areas(area);
    let highlight = Style::new().bg(Color::Indexed(236)).fg(ACCENT);

    // Waiting devices.
    let block = card("Waiting to pair");
    match &state.pending {
        Pending::Ready(list) if !list.is_empty() => {
            let rows: Vec<Row> = list
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let r = Row::new(vec![
                        Cell::from(if p.name.is_empty() {
                            "Unknown device".into()
                        } else {
                            p.name.clone()
                        })
                        .bold(),
                        Cell::from(Span::styled(p.address.clone(), muted())),
                    ]);
                    if i == state.selected {
                        r.style(highlight)
                    } else {
                        r
                    }
                })
                .collect();
            let table = Table::new(rows, [Constraint::Percentage(50), Constraint::Fill(1)])
                .block(block.title_bottom(
                    hints(&[("p", "pair"), ("c", "cancel request")]).right_aligned(),
                ));
            f.render_widget(table, pending_area);
        }
        Pending::Ready(_) => f.render_widget(
            Paragraph::new(vec![
                Line::styled("No devices are waiting to pair.", muted()),
                Line::styled(
                    "Add this computer in Moonlight; it will show up here with a PIN to enter.",
                    muted(),
                ),
            ])
            .block(block),
            pending_area,
        ),
        Pending::Legacy => f.render_widget(
            Paragraph::new(vec![
                Line::raw("Enter the PIN Moonlight shows to pair it."),
                hints(&[("p", "pair with PIN")]),
            ])
            .block(block),
            pending_area,
        ),
        Pending::Failed(e) => f.render_widget(
            Paragraph::new(Line::styled(
                format!("Couldn't load pairing requests: {e}"),
                Style::new().fg(BAD),
            ))
            .block(block),
            pending_area,
        ),
        Pending::Loading => f.render_widget(
            Paragraph::new(Line::styled("Loading…", muted())).block(block),
            pending_area,
        ),
    }

    // Paired clients.
    let offset = state.pending().len();
    match &state.clients {
        Load::Ready(clients) => {
            let has_toggle = clients.iter().any(|c| c.enabled().is_some());
            let rows: Vec<Row> = clients
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let status = match c.enabled() {
                        Some(true) => Span::styled("● enabled", Style::new().fg(OK)),
                        Some(false) => Span::styled("○ disabled", muted()),
                        None => Span::raw(""),
                    };
                    let created = c.created().map(format_date).unwrap_or_else(|| "—".into());
                    let dup = clients.iter().filter(|o| o.name == c.name).count() > 1;
                    let mut name = vec![Span::styled(display_name(c), Style::new().bold())];
                    if dup {
                        name.push(Span::styled("  same name", Style::new().fg(ACCENT)));
                    }
                    let r = Row::new(vec![
                        Cell::from(Line::from(name)),
                        Cell::from(status),
                        Cell::from(created),
                        Cell::from(Span::styled(c.uuid.clone(), muted())),
                    ]);
                    if i + offset == state.selected {
                        r.style(highlight)
                    } else {
                        r
                    }
                })
                .collect();
            let mut keys = vec![("d", "unpair"), ("D", "unpair all")];
            if has_toggle {
                keys.insert(0, ("Space", "enable/disable"));
            }
            let title = format!("Paired devices ({})", clients.len());
            let header = Row::new(["Name", "Status", "ID created", "UUID"]).style(muted());
            let table = Table::new(
                rows,
                [
                    Constraint::Percentage(35),
                    Constraint::Length(11),
                    Constraint::Length(11),
                    Constraint::Fill(1),
                ],
            )
            .header(header)
            .block(card(&title).title_bottom(hints(&keys).right_aligned()));
            f.render_widget(table, clients_area);
            if clients.is_empty() {
                f.render_widget(
                    Line::styled("No paired devices.", muted()),
                    clients_area.inner(Margin::new(2, 1)),
                );
            }
        }
        Load::Failed(e) => f.render_widget(
            Paragraph::new(Line::styled(
                format!("Couldn't load paired devices: {e}"),
                Style::new().fg(BAD),
            ))
            .block(card("Paired devices")),
            clients_area,
        ),
        _ => f.render_widget(
            Paragraph::new(Line::styled("Loading…", muted())).block(card("Paired devices")),
            clients_area,
        ),
    }
}
