//! System screen: troubleshooting actions, permissions and the admin account.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, Wrap};
use sun_control::api::types::Permission;
use sun_control::api::{ApiError, Credentials};
use sun_control::devices::format_date;

use super::app::{Action, App, Confirm, FormKind, Load, Modal, Msg, ToastKind};
use super::ui::{ACCENT, BAD, OK, card, muted, not_connected, pane};
use super::widgets::{Field, Form, Input};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysAction {
    Restart,
    CloseApp,
    ResetPortal,
    ResetDisplay,
    ExportLogs,
    ChangeLogin,
}

impl SysAction {
    /// Actions that apply to `platform`, mirroring the web UI.
    pub fn for_platform(platform: Option<&str>) -> Vec<SysAction> {
        let mut list = vec![SysAction::Restart, SysAction::CloseApp];
        match platform {
            Some("linux" | "freebsd") => list.push(SysAction::ResetPortal),
            Some("windows") => list.push(SysAction::ResetDisplay),
            _ => {}
        }
        list.extend([SysAction::ExportLogs, SysAction::ChangeLogin]);
        list
    }

    pub fn title(self) -> &'static str {
        match self {
            SysAction::Restart => "Restart Sunshine",
            SysAction::CloseApp => "Force close the running app",
            SysAction::ResetPortal => "Reset XDG portal capture",
            SysAction::ResetDisplay => "Reset persistent display settings",
            SysAction::ExportLogs => "Export logs to a file",
            SysAction::ChangeLogin => "Change admin username or password",
        }
    }

    fn desc(self) -> &'static str {
        match self {
            SysAction::Restart => {
                "If Sunshine isn't working properly, try restarting it. This ends any running sessions."
            }
            SysAction::CloseApp => {
                "If Moonlight complains that an app is already running, force closing it should fix the issue."
            }
            SysAction::ResetPortal => {
                "Clears the saved XDG portal restore token and restarts Sunshine. You'll be asked to pick a display and grant permission again. This ends any running sessions."
            }
            SysAction::ResetDisplay => {
                "If Sunshine is stuck trying to restore changed display settings, reset them and restore the display state manually."
            }
            SysAction::ExportLogs => {
                "Save Sunshine's current log to a file on this machine, e.g. to attach to a bug report."
            }
            SysAction::ChangeLogin => {
                "Change the username and password for Sunshine's admin interface. sun-control reconnects with the new login."
            }
        }
    }
}

pub enum Permissions {
    Loading,
    Ready(Vec<Permission>),
    Unsupported,
    Failed(String),
}

pub struct SystemState {
    pub action_sel: usize,
    pub perm_sel: usize,
    pub permissions: Permissions,
}

impl Default for SystemState {
    fn default() -> Self {
        Self {
            action_sel: 0,
            perm_sel: 0,
            permissions: Permissions::Loading,
        }
    }
}

impl SystemState {
    fn permissions(&self) -> &[Permission] {
        match &self.permissions {
            Permissions::Ready(p) => p,
            _ => &[],
        }
    }
}

/// `~/sunshine-<host>-YYYYMMDD-HHMMSS.log`, in UTC.
fn default_log_path(host: &str) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let date = format_date(secs).replace('-', "");
    let t = secs.rem_euclid(86_400);
    let host: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "~/sunshine-{host}-{date}-{:02}{:02}{:02}.log",
        t / 3600,
        t / 60 % 60,
        t % 60
    )
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => directories::BaseDirs::new()
            .map_or_else(|| PathBuf::from(path), |b| b.home_dir().join(rest)),
        None => PathBuf::from(path),
    }
}

impl App {
    pub(super) fn platform_name(&self) -> Option<String> {
        match &self.info {
            Load::Ready(i) => i.platform.clone(),
            _ => None,
        }
    }

    fn system_actions(&self) -> Vec<SysAction> {
        SysAction::for_platform(self.platform_name().as_deref())
    }

    pub(super) fn load_system(&mut self) {
        let Some(client) = self.client().cloned() else {
            return;
        };
        self.spawn(async move { Msg::Permissions(client.permissions().await) });
    }

    pub(super) fn on_permissions(&mut self, result: Result<Vec<Permission>, ApiError>) {
        self.system.permissions = match result {
            Ok(p) => Permissions::Ready(p),
            Err(ApiError::Unsupported(_)) => Permissions::Unsupported,
            Err(e) => Permissions::Failed(e.to_string()),
        };
        let n = self.system.permissions().len();
        self.system.perm_sel = self.system.perm_sel.min(n.saturating_sub(1));
    }

    pub(super) fn system_key(&mut self, key: KeyEvent) {
        if self.client().is_none() {
            return;
        }
        let actions = self.system_actions();
        let perms = self.system.permissions().len();
        match (self.focus, key.code) {
            (1, KeyCode::Down | KeyCode::Char('j')) => {
                self.system.action_sel = (self.system.action_sel + 1).min(actions.len() - 1)
            }
            (1, KeyCode::Up | KeyCode::Char('k')) => {
                self.system.action_sel = self.system.action_sel.saturating_sub(1)
            }
            (1, KeyCode::Enter | KeyCode::Char(' ')) => {
                if let Some(a) = actions.get(self.system.action_sel) {
                    self.start_system_action(*a);
                }
            }
            (2, KeyCode::Down | KeyCode::Char('j')) => {
                self.system.perm_sel = (self.system.perm_sel + 1).min(perms.saturating_sub(1))
            }
            (2, KeyCode::Up | KeyCode::Char('k')) => {
                self.system.perm_sel = self.system.perm_sel.saturating_sub(1)
            }
            _ => {}
        }
    }

    pub(super) fn start_system_action(&mut self, sys: SysAction) {
        let confirm = |title: &str, action: Action| {
            Some(Modal::Confirm(Confirm {
                title: title.into(),
                body: sys.desc().into(),
                action,
            }))
        };
        self.modal = match sys {
            SysAction::Restart => confirm("Restart Sunshine?", Action::Restart),
            SysAction::CloseApp => confirm("Force close the running app?", Action::CloseApp),
            SysAction::ResetPortal => confirm("Reset XDG portal capture?", Action::ResetPortal),
            SysAction::ResetDisplay => {
                confirm("Reset persistent display settings?", Action::ResetDisplay)
            }
            SysAction::ExportLogs => {
                let host = self.current_host().unwrap_or("sunshine").to_owned();
                Some(Modal::Form(
                    Form::new(
                        "Export logs",
                        FormKind::ExportLogs,
                        vec![
                            Field::new("Save to", Input::new(default_log_path(&host)))
                                .hint("on this machine"),
                        ],
                    )
                    .note("Existing files are not overwritten."),
                ))
            }
            SysAction::ChangeLogin => {
                let user = self
                    .client()
                    .and_then(|c| c.credentials().map(|c| c.username.clone()))
                    .unwrap_or_default();
                Some(Modal::Form(
                    Form::new(
                        "Change admin login",
                        FormKind::ChangeLogin,
                        vec![
                            Field::new("Current username", Input::new(user.clone())),
                            Field::new("Current password", Input::masked()),
                            Field::new("New username", Input::new(user)),
                            Field::new("New password", Input::masked()),
                            Field::new("Confirm new password", Input::masked()),
                        ],
                    )
                    .note("This changes the login for Sunshine's web UI and for sun-control."),
                ))
            }
        };
    }

    pub(super) fn submit_export_logs(&mut self, form: Form<FormKind>) {
        let path = expand_home(form.value(0));
        if path.as_os_str().is_empty() {
            self.toast("Enter a file path", ToastKind::Error);
            self.modal = Some(Modal::Form(form));
            return;
        }
        self.run_api(move |c| async move {
            let log = c.logs().await?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| ApiError::Local(format!("Can't write {}: {e}", path.display())))?;
            std::io::Write::write_all(&mut file, log.as_bytes())
                .map_err(|e| ApiError::Local(format!("Can't write {}: {e}", path.display())))?;
            Ok(format!(
                "Saved {} log lines to {}",
                log.lines().count(),
                path.display()
            ))
        });
    }

    pub(super) fn submit_change_login(&mut self, form: Form<FormKind>) {
        let field = |i: usize| form.fields[i].input.value.clone();
        let (cur_user, cur_pass, new_user, new_pass, confirm) =
            (field(0), field(1), field(2), field(3), field(4));
        let problem = if cur_user.trim().is_empty() || new_user.trim().is_empty() {
            Some("Usernames can't be empty")
        } else if new_pass.is_empty() {
            Some("Enter a new password")
        } else if new_pass != confirm {
            Some("The new passwords don't match")
        } else {
            None
        };
        if let Some(p) = problem {
            self.toast(p, ToastKind::Error);
            self.modal = Some(Modal::Form(form));
            return;
        }
        let Some(client) = self.client().cloned() else {
            return;
        };
        let Some(host) = self.current_host().map(str::to_owned) else {
            return;
        };
        let current = Credentials {
            username: cur_user.trim().to_owned(),
            password: cur_pass,
        };
        let new = Credentials {
            username: new_user.trim().to_owned(),
            password: new_pass,
        };
        self.spawn(async move {
            let result = client
                .change_credentials(&current, &new)
                .await
                .map(|()| new);
            Msg::LoginChanged { host, result }
        });
    }

    /// Only called for the session that made the change (stale sessions are
    /// dropped), so `host` is the one we're still connected to.
    pub(super) fn on_login_changed(&mut self, host: String, result: Result<Credentials, ApiError>) {
        match result {
            Ok(new) => {
                if let Some(h) = self.profiles.hosts.get_mut(&host)
                    && h.username.is_some()
                {
                    h.username = Some(new.username.clone());
                    let _ = self.profiles.save();
                }
                self.toast("Admin login changed; reconnecting…", ToastKind::Info);
                self.reconnect_with(host, new);
            }
            Err(e) => self.toast(format!("Couldn't change the login: {e}"), ToastKind::Error),
        }
    }
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    if app.client().is_none() {
        return not_connected(f, app, area);
    }
    let area = area.inner(Margin::new(1, 0));
    let platform = app.platform_name();
    let actions = SysAction::for_platform(platform.as_deref());
    let state = &app.system;
    let [actions_area, desc_area, perms_area] = Layout::vertical([
        Constraint::Length(actions.len() as u16 + 2),
        Constraint::Length(4),
        Constraint::Min(5),
    ])
    .areas(area);

    let lines: Vec<Line> = actions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            if app.focus == 1 && i == state.action_sel {
                Line::from(vec![
                    Span::styled("› ", Style::new().fg(ACCENT)),
                    Span::styled(a.title(), Style::new().fg(ACCENT).bold()),
                ])
                .style(Style::new().bg(Color::Indexed(236)))
            } else {
                Line::raw(format!("  {}", a.title()))
            }
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(pane("Actions", app.focus == 1)),
        actions_area,
    );

    let selected = actions
        .get(state.action_sel)
        .copied()
        .unwrap_or(SysAction::Restart);
    f.render_widget(
        Paragraph::new(selected.desc())
            .style(muted())
            .block(card("About this action"))
            .wrap(Wrap { trim: true }),
        desc_area,
    );

    let block = pane("Permissions", app.focus == 2);
    match &state.permissions {
        Permissions::Ready(list) if !list.is_empty() => {
            let [table_area, help_area] =
                Layout::vertical([Constraint::Min(3), Constraint::Length(4)])
                    .areas(block.inner(perms_area));
            f.render_widget(block, perms_area);
            let rows: Vec<Row> = list
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let status = match p.status.as_str() {
                        "granted" => Span::styled("● granted", Style::new().fg(OK)),
                        "denied" => Span::styled("✕ not granted", Style::new().fg(BAD)),
                        "not_determined" => {
                            Span::styled("○ not requested", Style::new().fg(ACCENT))
                        }
                        "on_use" => Span::styled("asked when used", muted()),
                        other => Span::styled(
                            if other.is_empty() {
                                "unknown".to_owned()
                            } else {
                                other.replace('_', " ")
                            },
                            muted(),
                        ),
                    };
                    let need = if p.is_required() {
                        "required"
                    } else {
                        "optional"
                    };
                    let row = Row::new(vec![
                        Cell::from(p.label(platform.as_deref())).bold(),
                        Cell::from(status),
                        Cell::from(Span::styled(need, muted())),
                    ]);
                    if app.focus == 2 && i == state.perm_sel {
                        row.style(Style::new().bg(Color::Indexed(236)))
                    } else {
                        row
                    }
                })
                .collect();
            let header = Row::new(["Permission", "Status", "Requirement"]).style(muted());
            f.render_widget(
                Table::new(
                    rows,
                    [
                        Constraint::Percentage(45),
                        Constraint::Length(18),
                        Constraint::Fill(1),
                    ],
                )
                .header(header),
                table_area,
            );
            let help = list
                .get(state.perm_sel)
                .and_then(|p| {
                    p.help(platform.as_deref())
                        .filter(|_| p.status != "granted")
                })
                .unwrap_or("Sunshine restarts by itself once it detects all required access.");
            f.render_widget(
                Paragraph::new(help)
                    .style(muted())
                    .wrap(Wrap { trim: true }),
                help_area,
            );
        }
        Permissions::Ready(_) => f.render_widget(
            Paragraph::new(Line::styled(
                "Sunshine reports no special permissions to check.",
                muted(),
            ))
            .block(block),
            perms_area,
        ),
        Permissions::Unsupported => f.render_widget(
            Paragraph::new(Line::styled(
                "This Sunshine version doesn't report permissions.",
                muted(),
            ))
            .block(block),
            perms_area,
        ),
        Permissions::Failed(e) => f.render_widget(
            Paragraph::new(Line::styled(
                format!("Couldn't check permissions: {e}"),
                Style::new().fg(BAD),
            ))
            .block(block),
            perms_area,
        ),
        Permissions::Loading => f.render_widget(
            Paragraph::new(Line::styled("Loading…", muted())).block(block),
            perms_area,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_follow_platform() {
        assert!(SysAction::for_platform(Some("linux")).contains(&SysAction::ResetPortal));
        assert!(!SysAction::for_platform(Some("linux")).contains(&SysAction::ResetDisplay));
        assert!(SysAction::for_platform(Some("windows")).contains(&SysAction::ResetDisplay));
    }

    #[test]
    fn log_path_is_safe() {
        let p = default_log_path("http://127.0.0.1:47999");
        assert!(p.starts_with("~/sunshine-http___127_0_0_1_47999-"), "{p}");
        assert!(p.ends_with(".log"));
    }
}
