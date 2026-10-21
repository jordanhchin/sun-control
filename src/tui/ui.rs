//! Rendering.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Cell, Clear, Padding, Paragraph, Row, Table, Wrap};

use super::app::{App, ConnState, FormKind, Load, Modal, Screen, ToastKind};
use super::logs::{Level, LogStatus};
use super::widgets::{Form, centered};

pub(super) const ACCENT: Color = Color::Yellow;
pub(super) const MUTED: Color = Color::DarkGray;
pub(super) const OK: Color = Color::Green;
pub(super) const BAD: Color = Color::Red;

pub(super) fn muted() -> Style {
    Style::new().fg(MUTED)
}

pub(super) fn key(k: &str) -> Span<'_> {
    Span::styled(k, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
}

pub(super) fn hints<'a>(pairs: &[(&'a str, &'a str)]) -> Line<'a> {
    let mut spans = vec![Span::raw(" ")];
    for (i, (k, label)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", muted()));
        }
        spans.push(key(k));
        spans.push(Span::styled(format!(" {label}"), muted()));
    }
    Line::from(spans)
}

pub(super) fn card(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(muted())
        .title(Span::styled(format!(" {title} "), Style::new().bold()))
        .padding(Padding::horizontal(1))
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let [nav, main] = Layout::horizontal([Constraint::Length(16), Constraint::Min(0)]).areas(body);

    draw_header(f, app, header);
    draw_nav(f, app, nav);
    match app.screen {
        Screen::Dashboard => draw_dashboard(f, app, main),
        Screen::Apps => super::apps::draw(f, app, main),
        Screen::Pairing => super::pairing::draw(f, app, main),
        Screen::Logs => draw_logs(f, app, main),
        Screen::Hosts => draw_hosts(f, app, main),
    }
    draw_footer(f, app, footer);

    match &mut app.modal {
        Some(Modal::Editor(editor)) => editor.draw(f),
        Some(Modal::Help) => draw_help(f),
        Some(Modal::Confirm(c)) => {
            let body = Text::from(c.body.as_str());
            let width = 64;
            let height = body.height() as u16 + 6;
            let area = centered(f.area(), width, height);
            f.render_widget(Clear, area);
            let block = modal_block(&c.title);
            let inner = block.inner(area);
            f.render_widget(block, area);
            let [text, buttons] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
            f.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), text);
            f.render_widget(
                hints(&[("y", "confirm"), ("n", "cancel")]).alignment(Alignment::Right),
                buttons,
            );
        }
        Some(Modal::Form(form)) => draw_form(f, form),
        None => {}
    }
}

pub(super) fn modal_block(title: &str) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(ACCENT).bold(),
        ))
        .padding(Padding::new(2, 2, 1, 0))
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let (dot, state) = match &app.conn {
        ConnState::Connected(_) => (
            Span::styled("●", Style::new().fg(OK)),
            "connected".to_owned(),
        ),
        ConnState::Connecting(_) => (
            Span::styled("◌", Style::new().fg(ACCENT)),
            "connecting…".to_owned(),
        ),
        ConnState::Failed { .. } => (
            Span::styled("●", Style::new().fg(BAD)),
            "not connected".to_owned(),
        ),
        ConnState::Disconnected => (Span::styled("○", muted()), "no host".to_owned()),
    };
    let mut spans = vec![
        Span::styled(
            " ☀ sun-control ",
            Style::new().fg(Color::Black).bg(ACCENT).bold(),
        ),
        Span::raw("  "),
        dot,
        Span::raw(" "),
    ];
    if let Some(h) = app.current_host() {
        spans.push(Span::styled(h.to_owned(), Style::new().bold()));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(state, muted()));
    if let (Some(c), Load::Ready(info)) = (app.client(), &app.info) {
        let right = format!(
            "Sunshine {} · {} ",
            info.version.as_deref().unwrap_or("?"),
            c.base_url().host_str().unwrap_or("")
        );
        f.render_widget(
            Line::styled(right, muted()).alignment(Alignment::Right),
            area,
        );
    }
    f.render_widget(Line::from(spans), area);
}

fn draw_nav(f: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![Line::raw("")];
    for (i, s) in Screen::ALL.iter().enumerate() {
        let active = *s == app.screen;
        let marker = if active { "▌" } else { " " };
        let style = if active {
            Style::new().fg(ACCENT).bold()
        } else {
            Style::new()
        };
        lines.push(Line::from(vec![
            Span::styled(marker, Style::new().fg(ACCENT)),
            Span::styled(format!("{} ", i + 1), muted()),
            Span::styled(s.title(), style),
        ]));
    }
    let block = Block::new()
        .borders(ratatui::widgets::Borders::RIGHT)
        .border_style(muted());
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    if let Some(t) = &app.toast {
        let style = match t.kind {
            ToastKind::Info => Style::new().fg(OK),
            ToastKind::Error => Style::new().fg(BAD),
        };
        f.render_widget(Line::styled(format!(" {}", t.text), style), area);
        return;
    }
    let line = match app.screen {
        Screen::Dashboard if app.client().is_some() => hints(&[
            ("r", "restart"),
            ("x", "close app"),
            ("R", "refresh"),
            ("Tab", "next"),
            ("?", "help"),
            ("q", "quit"),
        ]),
        Screen::Dashboard => hints(&[
            ("Enter", "retry"),
            ("5", "hosts"),
            ("?", "help"),
            ("q", "quit"),
        ]),
        Screen::Apps => hints(&[
            ("Enter", "edit"),
            ("a", "add"),
            ("c", "duplicate"),
            ("d", "delete"),
            ("J/K", "move"),
            ("?", "help"),
            ("q", "quit"),
        ]),
        Screen::Pairing => hints(&[
            ("↑↓", "select"),
            ("p", "pair"),
            ("Enter", "pair/toggle"),
            ("d", "unpair/cancel"),
            ("?", "help"),
            ("q", "quit"),
        ]),
        Screen::Logs => hints(&[
            ("↑↓", "scroll"),
            ("f", "follow"),
            ("l", "level"),
            ("/", "search"),
            ("n/N", "next/prev"),
            ("g/G", "top/end"),
            ("q", "quit"),
        ]),
        Screen::Hosts => hints(&[
            ("Enter", "connect"),
            ("a", "add"),
            ("d", "remove"),
            ("s", "set default"),
            ("t", "re-trust"),
            ("q", "quit"),
        ]),
    };
    f.render_widget(line, area);
}

pub(super) fn not_connected(f: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line> = match &app.conn {
        ConnState::Connecting(h) => vec![Line::from(format!("Connecting to {h}…"))],
        ConnState::Failed { host, error } => vec![
            Line::styled(
                format!("Can't connect to {host}"),
                Style::new().fg(BAD).bold(),
            ),
            Line::raw(""),
            Line::styled(error.clone(), muted()),
            Line::raw(""),
            hints(&[("Enter", "retry"), ("5", "choose another host")]),
        ],
        _ => vec![
            Line::from("No host selected."),
            Line::raw(""),
            hints(&[("5", "choose a host")]),
        ],
    };
    let area = centered(
        area,
        area.width.saturating_sub(4).min(80),
        lines.len() as u16 + 2,
    );
    f.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}

pub(super) fn row<'a>(label: &'a str, value: impl Into<Span<'a>>) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{label:<16}"), muted()),
        value.into(),
    ])
}

fn count(n: Option<usize>) -> Span<'static> {
    match n {
        Some(n) => Span::styled(n.to_string(), Style::new().bold()),
        None => Span::styled("unavailable", muted()),
    }
}

fn draw_dashboard(f: &mut Frame, app: &App, area: Rect) {
    let Some(client) = app.client() else {
        return not_connected(f, app, area);
    };
    // Cards read badly when stretched across very wide terminals.
    let area = area.inner(ratatui::layout::Margin::new(1, 0));
    let area = Rect {
        width: area.width.min(110),
        ..area
    };
    let [top, actions, _] = Layout::vertical([
        Constraint::Length(8),
        Constraint::Length(7),
        Constraint::Min(0),
    ])
    .areas(area);
    let [server, overview] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(top);

    let info = match &app.info {
        Load::Ready(i) => Some(i),
        Load::Failed(e) => {
            let p = Paragraph::new(vec![
                Line::styled("Couldn't load server details", Style::new().fg(BAD).bold()),
                Line::styled(e.clone(), muted()),
            ])
            .block(card("Server"))
            .wrap(Wrap { trim: true });
            f.render_widget(p, top);
            None
        }
        _ => {
            f.render_widget(
                Paragraph::new(Line::styled("Loading…", muted())).block(card("Server")),
                top,
            );
            None
        }
    };

    if let Some(info) = info {
        let fp = info
            .fingerprint
            .as_deref()
            .map(|fp| format!("{}…", &fp[..fp.len().min(23)]));
        let server_lines = vec![
            row("Host", app.current_host().unwrap_or("").to_owned()),
            row("Web UI", client.base_url().to_string()),
            row(
                "Version",
                Span::styled(
                    info.version.clone().unwrap_or("unknown".into()),
                    Style::new().bold(),
                ),
            ),
            row(
                "Platform",
                info.platform.clone().unwrap_or("unknown".into()),
            ),
            row(
                "Certificate",
                Span::styled(fp.unwrap_or("not pinned".into()), muted()),
            ),
        ];
        f.render_widget(Paragraph::new(server_lines).block(card("Server")), server);

        let overview_lines = vec![
            row("Applications", count(info.apps)),
            row("Paired clients", count(info.clients)),
            row("Custom settings", count(Some(info.settings))),
        ];
        f.render_widget(
            Paragraph::new(overview_lines).block(card("Overview")),
            overview,
        );
    }

    let action_lines = vec![
        Line::from(vec![key(" r "), Span::raw(" Restart Sunshine")]),
        Line::from(vec![key(" x "), Span::raw(" Close the running app")]),
        Line::from(vec![key(" R "), Span::raw(" Refresh")]),
        Line::raw(""),
        Line::styled("Configuration editing is coming next.", muted()),
    ];
    f.render_widget(Paragraph::new(action_lines).block(card("Actions")), actions);
}

fn level_style(level: Level) -> Style {
    match level {
        Level::Verbose | Level::Debug => muted(),
        Level::Info => Style::new(),
        Level::Warning => Style::new().fg(ACCENT),
        Level::Error | Level::Fatal => Style::new().fg(BAD),
    }
}

/// Split a line into spans, highlighting case-insensitive matches of `term`.
fn highlight<'a>(text: &'a str, term: Option<&str>, base: Style) -> Line<'a> {
    let Some(term) = term.filter(|t| !t.is_empty()) else {
        return Line::styled(text, base);
    };
    let lower = text.to_lowercase();
    let needle = term.to_lowercase();
    // Lowercasing can change byte lengths for non-ASCII text; fall back to plain.
    if lower.len() != text.len() {
        return Line::styled(text, base);
    }
    let mut spans = Vec::new();
    let mut last = 0;
    for (i, _) in lower.match_indices(&needle) {
        spans.push(Span::styled(&text[last..i], base));
        spans.push(Span::styled(
            &text[i..i + needle.len()],
            Style::new().fg(Color::Black).bg(ACCENT),
        ));
        last = i + needle.len();
    }
    spans.push(Span::styled(&text[last..], base));
    Line::from(spans)
}

fn draw_logs(f: &mut Frame, app: &mut App, area: Rect) {
    if app.client().is_none() {
        return not_connected(f, app, area);
    }
    let logs = &mut app.logs;
    let mode = if logs.follow {
        Span::styled("● following", Style::new().fg(OK))
    } else {
        Span::styled("❚❚ paused", Style::new().fg(ACCENT))
    };
    let mut title = vec![
        Span::styled(" Logs ", Style::new().bold()),
        Span::styled(format!("· level ≥ {} · ", logs.min_level.label()), muted()),
        mode,
        Span::raw(" "),
    ];
    if let Some(s) = &logs.search {
        title.push(Span::styled(format!("· search “{s}” "), muted()));
    }
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(muted())
        .title(Line::from(title));
    let inner = block.inner(area);
    logs.view_height = inner.height as usize;

    let message = match &logs.status {
        LogStatus::Loading => Some(Line::styled("Loading…", muted())),
        LogStatus::Unsupported => Some(Line::styled(
            "This Sunshine version doesn't expose logs over the API.",
            muted(),
        )),
        LogStatus::Failed(e) => Some(Line::styled(
            format!("Couldn't load logs: {e}"),
            Style::new().fg(BAD),
        )),
        LogStatus::Loaded if logs.lines.is_empty() => Some(Line::styled(
            "Sunshine returned an empty log. New entries will appear here as they're written.",
            muted(),
        )),
        LogStatus::Loaded => None,
    };
    if let Some(m) = message {
        f.render_widget(Paragraph::new(m).block(block), area);
        return;
    }

    let visible = logs.visible();
    let total = visible.len();
    let first = logs.first_row(total);
    let search = logs.search.as_deref();
    let lines: Vec<Line> = visible
        .iter()
        .skip(first)
        .take(inner.height as usize)
        .map(|l| highlight(&l.text, search, level_style(l.level)))
        .collect();
    let position = if total == 0 {
        "no lines at this level".to_owned()
    } else {
        format!(
            "{}–{} of {total}",
            first + 1,
            (first + inner.height as usize).min(total)
        )
    };
    let block = block
        .title_bottom(Line::styled(format!(" {position} "), muted()).alignment(Alignment::Right));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_hosts(f: &mut Frame, app: &mut App, area: Rect) {
    let area = area.inner(ratatui::layout::Margin::new(1, 0));
    let [table_area, detail] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(7)]).areas(area);
    let default = app.profiles.default_name().to_owned();
    let current = app.current_host().map(str::to_owned);

    let rows: Vec<Row> = app
        .profiles
        .hosts
        .iter()
        .map(|(name, h)| {
            let mark = if Some(name) == current.as_ref() {
                Span::styled("●", Style::new().fg(OK))
            } else {
                Span::raw(" ")
            };
            let def = if *name == default {
                Span::styled("default", Style::new().fg(ACCENT))
            } else {
                Span::raw("")
            };
            let cert = if h.fingerprint.is_some() {
                Span::styled("pinned", Style::new().fg(OK))
            } else {
                Span::styled("unverified", muted())
            };
            Row::new(vec![
                Cell::from(mark),
                Cell::from(name.clone()).bold(),
                Cell::from(h.url.clone()),
                Cell::from(h.username.clone().unwrap_or_default()),
                Cell::from(cert),
                Cell::from(def),
            ])
        })
        .collect();
    let header = Row::new(["", "Name", "Address", "User", "Certificate", ""]).style(muted());
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(16),
            Constraint::Min(24),
            Constraint::Length(12),
            Constraint::Length(11),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(card("Hosts"))
    .row_highlight_style(Style::new().bg(Color::Indexed(236)).fg(ACCENT))
    .highlight_symbol("› ");
    f.render_stateful_widget(table, table_area, &mut app.hosts);

    let selected = app
        .hosts
        .selected()
        .and_then(|i| app.profiles.hosts.iter().nth(i));
    let lines = match selected {
        Some((name, h)) => vec![
            row("Name", name.clone()),
            row(
                "Password from",
                h.password_cmd.clone().unwrap_or("prompt".into()),
            ),
            row(
                "Fingerprint",
                Span::styled(
                    h.fingerprint.clone().unwrap_or("not pinned yet".into()),
                    muted(),
                ),
            ),
        ],
        None => vec![],
    };
    f.render_widget(
        Paragraph::new(lines)
            .block(card("Details"))
            .wrap(Wrap { trim: true }),
        detail,
    );
}

fn draw_form(f: &mut Frame, form: &Form<FormKind>) {
    // Blank spacer line plus the wrapped note (inner width is 64 - 2 borders - 4 padding).
    let note_height = form
        .note
        .as_ref()
        .map_or(0, |n| 1 + n.chars().count().div_ceil(58) as u16);
    let height = form.fields.len() as u16 * 2 + note_height + 5;
    let area = centered(f.area(), 64, height);
    f.render_widget(Clear, area);
    let block = modal_block(&form.title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();
    for (i, field) in form.fields.iter().enumerate() {
        let focused = i == form.focus;
        let label_style = if focused {
            Style::new().fg(ACCENT).bold()
        } else {
            muted()
        };
        lines.push(Line::from(vec![
            Span::styled(field.label, label_style),
            Span::styled(
                if field.hint.is_empty() {
                    String::new()
                } else {
                    format!("  {}", field.hint)
                },
                muted(),
            ),
        ]));
        let mut value = vec![Span::styled(
            if focused { "› " } else { "  " },
            Style::new().fg(ACCENT),
        )];
        value.extend(
            field
                .input
                .line(inner.width.saturating_sub(3) as usize, focused)
                .spans,
        );
        lines.push(Line::from(value));
    }
    if let Some(note) = &form.note {
        lines.push(Line::raw(""));
        lines.push(Line::styled(note.clone(), muted()));
    }
    let [body, buttons] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
    f.render_widget(
        hints(&[
            ("Enter", "next/submit"),
            ("Tab", "switch"),
            ("Esc", "cancel"),
        ])
        .alignment(Alignment::Right),
        buttons,
    );
}

fn draw_help(f: &mut Frame) {
    let section = |s: &'static str| Line::styled(s, Style::new().fg(ACCENT).bold());
    let item = |k: &'static str, d: &'static str| {
        Line::from(vec![
            Span::styled(format!("  {k:<16}"), Style::new().bold()),
            Span::styled(d, muted()),
        ])
    };
    let lines = vec![
        section("Global"),
        item("1-5 / Tab", "switch screen"),
        item("R", "refresh"),
        item("?", "this help"),
        item("q / Ctrl-C", "quit"),
        Line::raw(""),
        section("Dashboard"),
        item("r", "restart Sunshine"),
        item("x", "close the running app"),
        Line::raw(""),
        section("Apps"),
        item("Enter / e", "edit (Ctrl-S saves, Esc cancels)"),
        item("a / c / d", "add / duplicate / delete"),
        item("J / K", "move down / up"),
        Line::raw(""),
        section("Pairing"),
        item("p", "pair a waiting device with its PIN"),
        item("Enter / Space", "pair, or enable/disable a device"),
        item("d / D", "unpair (or cancel request) / unpair all"),
        Line::raw(""),
        section("Logs"),
        item("j/k ↑↓ PgUp/Dn", "scroll"),
        item("g / G", "top / end (follow)"),
        item("f", "toggle follow"),
        item("l", "cycle minimum level"),
        item("/ n N Esc", "search, next, previous, clear"),
        Line::raw(""),
        section("Hosts"),
        item("Enter", "connect"),
        item("a / d", "add / remove"),
        item("s", "make default"),
        item("t", "forget certificate and re-trust"),
    ];
    let area = centered(f.area(), 64, lines.len() as u16 + 4);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).block(
            modal_block("Keys").title_bottom(
                Line::styled(" any key to close ", muted()).alignment(Alignment::Right),
            ),
        ),
        area,
    );
}
