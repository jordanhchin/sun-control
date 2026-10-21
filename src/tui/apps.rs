//! Applications screen.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};
use sun_control::api::types::{App as SunApp, truthy};
use sun_control::api::{ApiError, SunshineClient};

use super::app::{Action, App, Confirm, Load, Modal, Msg, ToastKind};
use super::app_editor::AppEditor;
use super::ui::{ACCENT, card, muted, not_connected, pane, row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moving {
    No,
    /// The swap is being saved.
    Saving,
    /// Saved (or failed); reloading the list from the server.
    Reloading,
}

pub struct AppsState {
    pub list: Load<Vec<SunApp>>,
    pub table: TableState,
    /// A reorder is being saved or the list reloaded after it; app changes
    /// wait, since indices may not match the server.
    pub moving: Moving,
    /// Apps a failed reorder displaced, still to be offered for re-adding.
    pub recover: Vec<SunApp>,
    /// The editor for a recovery save that failed, waiting to be reshown.
    pub retry: Option<Box<AppEditor>>,
    /// A recovery save is in flight; the next one waits for its outcome.
    pub recovering: bool,
    /// The last app list requested; earlier responses are stale.
    pub request: u64,
}

/// Why a reorder failed, and any apps the server may no longer have.
#[derive(Debug)]
pub struct MoveFailure {
    pub message: String,
    pub lost: Vec<SunApp>,
    /// The server's list couldn't be read back, so `lost` is every app that
    /// might be missing rather than ones known to be.
    pub uncertain: bool,
}

impl MoveFailure {
    /// Judge a failure against the server's list (`None` if unreadable).
    fn new(e: &ApiError, list: Option<&[SunApp]>, apps: [&SunApp; 2]) -> Self {
        let lost = apps
            .into_iter()
            .filter(|x| list.is_none_or(|l| !l.iter().any(|y| same_app(x, y))))
            .cloned()
            .collect();
        Self {
            message: e.to_string(),
            lost,
            uncertain: list.is_none(),
        }
    }
}

/// Whether two entries are the same app, preferring Sunshine's uuid.
fn same_app(x: &SunApp, y: &SunApp) -> bool {
    match (x.extra.get("uuid"), y.extra.get("uuid")) {
        (Some(a), Some(b)) => a == b,
        _ => serde_json::to_value(x).ok() == serde_json::to_value(y).ok(),
    }
}

/// Put `a` at `to` and `b` at `from`.
///
/// A failed save may still have been applied (the response was lost or
/// malformed), so every failure is judged against what the server then has.
async fn swap_apps(
    client: &SunshineClient,
    (from, a): (usize, SunApp),
    (to, b): (usize, SunApp),
) -> Result<(), MoveFailure> {
    let server = || async { client.apps().await.ok().map(|l| l.apps) };
    let at = |l: &[SunApp], i: usize, x: &SunApp| l.get(i).is_some_and(|y| same_app(x, y));
    if let Err(e) = client.save_app(&a, Some(to)).await {
        let list = server().await;
        match list.as_deref() {
            // Applied despite the error: carry on with the second half.
            Some(l) if at(l, to, &a) && at(l, from, &a) => {}
            l => return Err(MoveFailure::new(&e, l, [&a, &b])),
        }
    }
    // `b` now only exists in memory until this succeeds.
    if let Err(e) = client.save_app(&b, Some(from)).await {
        let list = server().await;
        match list.as_deref() {
            Some(l) if at(l, to, &a) && at(l, from, &b) => return Ok(()),
            // Only the first half applied: put `b` back in its own slot.
            Some(l) if at(l, to, &a) && at(l, from, &a) => {
                let _ = client.save_app(&b, Some(to)).await;
                let list = server().await;
                return Err(MoveFailure::new(&e, list.as_deref(), [&a, &b]));
            }
            l => return Err(MoveFailure::new(&e, l, [&a, &b])),
        }
    }
    Ok(())
}

impl Default for AppsState {
    fn default() -> Self {
        Self {
            list: Load::Idle,
            table: TableState::default().with_selected(Some(0)),
            moving: Moving::No,
            recover: Vec::new(),
            retry: None,
            recovering: false,
            request: 0,
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
        // Supersede every earlier request, even if this one can't be sent.
        self.apps.request += 1;
        let request = self.apps.request;
        let Some(client) = self.client().cloned() else {
            return;
        };
        if !matches!(self.apps.list, Load::Ready(_)) {
            self.apps.list = Load::Loading;
        }
        self.spawn(async move { Msg::Apps(request, client.apps().await.map(|l| l.apps)) });
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
        let mutates = matches!(
            key.code,
            KeyCode::Char('J' | 'K' | 'a' | 'e' | 'c' | 'd') | KeyCode::Enter | KeyCode::Delete
        ) || (key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Up | KeyCode::Down));
        if mutates && self.apps.moving != Moving::No {
            return self.toast("Still saving the app order…", ToastKind::Info);
        }
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

    /// Sunshine has no reorder endpoint, so moving swaps two entries in place
    /// with two saves. If the second fails, the first is rolled back; if that
    /// fails too, the displaced app is handed back for re-adding.
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
        let Some(client) = self.client().cloned() else {
            return;
        };
        if let Load::Ready(list) = &mut self.apps.list {
            list.swap(from, to);
        }
        self.apps.table.select(Some(to));
        self.apps.moving = Moving::Saving;
        self.spawn(async move { Msg::AppMoved(swap_apps(&client, (from, a), (to, b)).await) });
    }

    /// `moving` stays set until the reload lands, so nothing acts on indices
    /// from the optimistic list.
    pub(super) fn on_app_moved(&mut self, result: Result<(), MoveFailure>) {
        if let Err(failure) = result {
            // The optimistic order is wrong; don't show or act on it.
            self.apps.list = Load::Loading;
            let MoveFailure {
                message,
                mut lost,
                uncertain,
            } = failure;
            if lost.is_empty() {
                self.toast(
                    format!("Couldn't move the app: {message}"),
                    ToastKind::Error,
                );
            } else {
                // Don't lose apps: offer each back as a new entry to save.
                let names: Vec<String> = lost.iter().map(|a| format!("“{}”", a.name)).collect();
                let names = names.join(" and ");
                let text = if uncertain {
                    format!(
                        "Moving failed ({message}) and the app list couldn't be checked. If {names} is missing, save to re-add it."
                    )
                } else {
                    format!("Moving failed ({message}). Save to re-add {names}.")
                };
                self.toast(text, ToastKind::Error);
                lost.reverse();
                self.apps.recover.extend(lost);
                self.next_recovered_app();
            }
        }
        // Whatever happened, show what the server actually has.
        self.apps.moving = Moving::Reloading;
        self.load_apps();
    }

    /// Open the next displaced app in the editor, if any and nothing else is
    /// on screen; a failed save's editor comes first, edits intact.
    pub(super) fn next_recovered_app(&mut self) {
        if self.modal.is_some() || self.apps.recovering {
            return;
        }
        if let Some(editor) = self.apps.retry.take() {
            self.modal = Some(Modal::Editor(editor));
            return;
        }
        // Left queued until the server confirms it has the app again.
        if let Some(app) = self.apps.recover.last().cloned() {
            let mut editor = AppEditor::new(app, None, self.platform().as_deref());
            editor.recovery = true;
            self.modal = Some(Modal::Editor(Box::new(editor)));
        }
    }

    fn save_recovered(&mut self, editor: Box<AppEditor>, app: SunApp) {
        let Some(client) = self.client().cloned() else {
            self.modal = Some(Modal::Editor(editor));
            return;
        };
        self.toast(format!("Saving “{}”…", app.name), ToastKind::Info);
        self.apps.recovering = true;
        self.spawn(async move {
            Msg::Recovered(match client.save_app(&app, None).await {
                Ok(()) => Ok(format!("Re-added “{}”", app.name)),
                Err(e) => Err((e.to_string(), editor)),
            })
        });
    }

    pub(super) fn on_recovered(&mut self, result: Result<String, (String, Box<AppEditor>)>) {
        self.apps.recovering = false;
        match result {
            Ok(text) => {
                self.apps.recover.pop();
                self.toast(text, ToastKind::Info);
                self.load_apps();
                self.next_recovered_app();
            }
            Err((e, mut editor)) => {
                // Hand the edits back for another try.
                self.toast(format!("Couldn't save: {e}"), ToastKind::Error);
                editor.error = Some(e);
                self.apps.retry = Some(editor);
                self.next_recovered_app();
            }
        }
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
        if editor.recovery {
            return self.save_recovered(editor, app);
        }
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
    .block(pane(&title, app.focus == 1))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;
    use sun_control::api::Credentials;
    use sun_control::api::tls::Trust;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    fn named(name: &str) -> SunApp {
        SunApp {
            name: name.into(),
            ..Default::default()
        }
    }

    /// What a save does: fail before applying, fail after applying (the
    /// response is lost or malformed), or succeed.
    #[derive(Clone, Copy)]
    enum Save {
        Ok,
        FailBefore,
        FailAfter,
    }

    /// A Sunshine app list that applies saves according to a script.
    struct Fake {
        apps: Mutex<Vec<String>>,
        script: Mutex<Vec<Save>>,
        list_works: bool,
    }

    impl Respond for Fake {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let mut apps = self.apps.lock().unwrap();
            if req.method.as_str() == "GET" {
                if !self.list_works {
                    return ResponseTemplate::new(500);
                }
                let list: Vec<_> = apps.iter().map(|n| json!({"name": n})).collect();
                return ResponseTemplate::new(200).set_body_json(json!({"apps": list}));
            }
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let step = self.script.lock().unwrap().remove(0);
            if !matches!(step, Save::FailBefore) {
                let i = body["index"].as_u64().unwrap() as usize;
                apps[i] = body["name"].as_str().unwrap().into();
            }
            match step {
                Save::Ok => ResponseTemplate::new(200).set_body_json(json!({"status": true})),
                _ => ResponseTemplate::new(200).set_body_string("<html>proxy error</html>"),
            }
        }
    }

    /// Swap A (index 0) and B (index 1); return the error and the server's final list.
    async fn swap(script: &[Save], list_works: bool) -> (Option<MoveFailure>, Vec<String>) {
        let server = MockServer::start().await;
        let fake = std::sync::Arc::new(Fake {
            apps: Mutex::new(vec!["A".into(), "B".into()]),
            script: Mutex::new(script.to_vec()),
            list_works,
        });
        struct Shared(std::sync::Arc<Fake>);
        impl Respond for Shared {
            fn respond(&self, req: &Request) -> ResponseTemplate {
                self.0.respond(req)
            }
        }
        Mock::given(path("/api/apps"))
            .respond_with(Shared(fake.clone()))
            .mount(&server)
            .await;
        let result = swap_apps(&client(&server), (0, named("A")), (1, named("B"))).await;
        let apps = fake.apps.lock().unwrap().clone();
        (result.err(), apps)
    }

    fn lost(f: &MoveFailure) -> Vec<&str> {
        f.lost.iter().map(|a| a.name.as_str()).collect()
    }

    fn client(server: &MockServer) -> SunshineClient {
        let creds = Credentials {
            username: "admin".into(),
            password: "pw".into(),
        };
        SunshineClient::new(
            format!("{}/", server.uri()).parse().unwrap(),
            Trust::Any,
            Some(creds),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn swaps_when_both_saves_succeed() {
        let (err, apps) = swap(&[Save::Ok, Save::Ok], true).await;
        assert!(err.is_none());
        assert_eq!(apps, ["B", "A"]);
    }

    #[tokio::test]
    async fn failed_first_save_changes_nothing() {
        let (err, apps) = swap(&[Save::FailBefore], true).await;
        assert!(err.unwrap().lost.is_empty());
        assert_eq!(apps, ["A", "B"]);
    }

    #[tokio::test]
    async fn first_save_applied_despite_error_still_completes() {
        let (err, apps) = swap(&[Save::FailAfter, Save::Ok], true).await;
        assert!(err.is_none(), "{err:?}");
        assert_eq!(apps, ["B", "A"]);
    }

    #[tokio::test]
    async fn second_save_applied_despite_error_is_kept() {
        let (err, apps) = swap(&[Save::Ok, Save::FailAfter], true).await;
        assert!(err.is_none(), "{err:?}");
        assert_eq!(apps, ["B", "A"], "rollback overwrote a completed swap");
    }

    #[tokio::test]
    async fn failed_second_save_is_rolled_back() {
        let (err, apps) = swap(&[Save::Ok, Save::FailBefore, Save::Ok], true).await;
        assert!(err.unwrap().lost.is_empty(), "B was restored");
        assert_eq!(apps, ["A", "B"]);
    }

    #[tokio::test]
    async fn failed_rollback_hands_back_the_displaced_app() {
        let (err, apps) = swap(&[Save::Ok, Save::FailBefore, Save::FailBefore], true).await;
        let err = err.unwrap();
        assert_eq!(lost(&err), ["B"]);
        assert!(!err.uncertain);
        assert_eq!(apps, ["A", "A"]);
    }

    #[tokio::test]
    async fn unreadable_state_keeps_both_apps_for_recovery() {
        let (err, _) = swap(&[Save::FailAfter], false).await;
        let err = err.unwrap();
        assert!(err.uncertain);
        assert_eq!(lost(&err), ["A", "B"]);
    }
}
