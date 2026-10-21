//! TUI state, key handling and background work.

use std::future::Future;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;
use sun_control::api::{ApiError, SunshineClient};
use sun_control::connect::{ConnectError, ConnectOptions, Connection, connect};
use sun_control::profiles::{self, Host, Profiles};
use tokio::sync::mpsc::UnboundedSender;

use super::app_editor::{AppEditor, EditorEvent};
use super::apps::AppsState;
use super::logs::LogsState;
use super::pairing::{PAIRING_REFRESH, PairingState};
use super::widgets::{Field, Form, FormEvent, Input};
use sun_control::api::types::{App as SunApp, Client, PendingPairing};

const DASHBOARD_REFRESH: Duration = Duration::from_secs(15);
const LOGS_REFRESH: Duration = Duration::from_secs(3);
const TOAST_TTL: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    Apps,
    Pairing,
    Logs,
    Hosts,
}

impl Screen {
    pub const ALL: [Screen; 5] = [
        Screen::Dashboard,
        Screen::Apps,
        Screen::Pairing,
        Screen::Logs,
        Screen::Hosts,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Screen::Dashboard => "Dashboard",
            Screen::Apps => "Apps",
            Screen::Pairing => "Pairing",
            Screen::Logs => "Logs",
            Screen::Hosts => "Hosts",
        }
    }
}

pub enum ConnState {
    Disconnected,
    Connecting(String),
    Connected(Connection),
    Failed { host: String, error: String },
}

#[derive(Debug, Clone, Default)]
pub struct Info {
    pub version: Option<String>,
    pub platform: Option<String>,
    pub settings: usize,
    pub apps: Option<usize>,
    pub clients: Option<usize>,
    pub fingerprint: Option<String>,
}

pub enum Load<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum Action {
    Restart,
    CloseApp,
    RemoveHost(String),
    Trust { host: String, fingerprint: String },
    DeleteApp { index: usize, name: String },
    Unpair { uuid: String, name: String },
    UnpairAll,
    CancelPairing { id: String, label: String },
}

pub struct Confirm {
    pub title: String,
    pub body: String,
    pub action: Action,
}

#[derive(Debug, Clone)]
pub enum FormKind {
    Credentials { host: String },
    AddHost,
    LogSearch,
    Pair { pairing_id: Option<String> },
}

pub enum Modal {
    Confirm(Confirm),
    Form(Form<FormKind>),
    Editor(Box<AppEditor>),
    Help,
}

pub enum ToastKind {
    Info,
    Error,
}

pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub at: Instant,
}

pub enum Msg {
    Connected {
        profiles: Box<Profiles>,
        opts: ConnectOptions,
        result: Result<Connection, ConnectError>,
    },
    Info(Result<Info, ApiError>),
    Logs(Result<String, ApiError>),
    Apps(Result<Vec<SunApp>, ApiError>),
    Pairing {
        pending: Result<Vec<PendingPairing>, ApiError>,
        clients: Result<Vec<Client>, ApiError>,
    },
    /// A finished action; an empty message refreshes without a toast.
    Done(Result<String, String>),
}

pub struct App {
    pub profiles: Profiles,
    pub insecure: bool,
    pub conn: ConnState,
    pub screen: Screen,
    pub info: Load<Info>,
    pub info_at: Option<Instant>,
    pub logs: LogsState,
    pub logs_at: Option<Instant>,
    pub apps: AppsState,
    pub pairing: PairingState,
    pub hosts: TableState,
    pub modal: Option<Modal>,
    pub toast: Option<Toast>,
    pub quit: bool,
    signed_in_as: Option<String>,
    pub(super) tx: UnboundedSender<Msg>,
}

impl App {
    pub fn new(profiles: Profiles, insecure: bool, tx: UnboundedSender<Msg>) -> Self {
        let mut hosts = TableState::default();
        hosts.select(Some(0));
        Self {
            profiles,
            insecure,
            conn: ConnState::Disconnected,
            screen: Screen::Dashboard,
            info: Load::Idle,
            info_at: None,
            logs: LogsState::default(),
            logs_at: None,
            apps: AppsState::default(),
            pairing: PairingState::default(),
            hosts,
            modal: None,
            toast: None,
            quit: false,
            signed_in_as: None,
            tx,
        }
    }

    pub fn client(&self) -> Option<&SunshineClient> {
        match &self.conn {
            ConnState::Connected(c) => Some(&c.client),
            _ => None,
        }
    }

    pub fn host_names(&self) -> Vec<String> {
        self.profiles.hosts.keys().cloned().collect()
    }

    pub fn current_host(&self) -> Option<&str> {
        match &self.conn {
            ConnState::Connected(c) => Some(&c.name),
            ConnState::Connecting(h) | ConnState::Failed { host: h, .. } => Some(h),
            ConnState::Disconnected => None,
        }
    }

    pub(super) fn toast(&mut self, text: impl Into<String>, kind: ToastKind) {
        self.toast = Some(Toast {
            text: text.into(),
            kind,
            at: Instant::now(),
        });
    }

    // ---- connection -------------------------------------------------------

    pub fn set_connected(&mut self, conn: Connection) {
        self.conn = ConnState::Connected(conn);
        self.info = Load::Idle;
        self.info_at = None;
        self.logs = LogsState::default();
        self.logs_at = None;
        self.apps = AppsState::default();
        self.pairing = PairingState::default();
        // Server details double as the credential check, whatever the screen.
        self.load_info();
        if self.screen != Screen::Dashboard {
            self.refresh();
        }
    }

    pub fn set_failed(&mut self, host: String, error: String) {
        self.conn = ConnState::Failed { host, error };
        self.info = Load::Idle;
    }

    pub fn connect_to(&mut self, host: &str) {
        let opts = ConnectOptions {
            host: Some(host.to_owned()),
            insecure: self.insecure,
            ..Default::default()
        };
        self.start_connect(opts);
    }

    fn start_connect(&mut self, opts: ConnectOptions) {
        let host = opts.host.clone().unwrap_or_default();
        self.conn = ConnState::Connecting(host);
        let mut profiles = self.profiles.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = connect(&mut profiles, &opts).await;
            let _ = tx.send(Msg::Connected {
                profiles: Box::new(profiles),
                opts,
                result,
            });
        });
    }

    // ---- data loading -----------------------------------------------------

    pub fn refresh(&mut self) {
        match self.screen {
            Screen::Logs => self.load_logs(),
            Screen::Apps => self.load_apps(),
            Screen::Pairing => self.load_pairing(),
            Screen::Dashboard | Screen::Hosts => self.load_info(),
        }
    }

    /// Run an API call in the background and report its result as a toast.
    pub(super) fn run_api<F, Fut>(&self, f: F)
    where
        F: FnOnce(SunshineClient) -> Fut,
        Fut: Future<Output = Result<String, ApiError>> + Send + 'static,
    {
        let Some(client) = self.client().cloned() else {
            return;
        };
        let tx = self.tx.clone();
        let fut = f(client);
        tokio::spawn(async move {
            let _ = tx.send(Msg::Done(fut.await.map_err(|e| e.to_string())));
        });
    }

    fn load_info(&mut self) {
        let Some(client) = self.client().cloned() else {
            return;
        };
        if matches!(self.info, Load::Idle | Load::Failed(_)) {
            self.info = Load::Loading;
        }
        self.info_at = Some(Instant::now());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(Msg::Info(fetch_info(&client).await));
        });
    }

    fn load_logs(&mut self) {
        if self.logs.in_flight {
            return;
        }
        let Some(client) = self.client().cloned() else {
            return;
        };
        self.logs.in_flight = true;
        self.logs_at = Some(Instant::now());
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(Msg::Logs(client.logs().await));
        });
    }

    fn run_action(&mut self, action: Action) {
        match action {
            Action::RemoveHost(name) => {
                self.profiles.hosts.remove(&name);
                if self.profiles.default.as_deref() == Some(&name) {
                    self.profiles.default = None;
                }
                self.save_profiles(format!("Removed '{name}'"));
                self.clamp_host_selection();
            }
            Action::Trust { host, fingerprint } => {
                let opts = ConnectOptions {
                    host: Some(host),
                    insecure: self.insecure,
                    accept_fingerprint: Some(fingerprint),
                    ..Default::default()
                };
                self.start_connect(opts);
            }
            Action::Restart => self.run_api(|c| async move {
                c.restart().await?;
                Ok("Sunshine is restarting".into())
            }),
            Action::CloseApp => self.run_api(|c| async move {
                c.close_app().await?;
                Ok("Closed the running app".into())
            }),
            Action::DeleteApp { index, name } => self.run_api(move |c| async move {
                c.delete_app(index).await?;
                Ok(format!("Deleted “{name}”"))
            }),
            Action::Unpair { uuid, name } => self.run_api(move |c| async move {
                c.unpair(&uuid).await?;
                Ok(format!("Unpaired “{name}”"))
            }),
            Action::UnpairAll => self.run_api(|c| async move {
                c.unpair_all().await?;
                Ok("Unpaired all devices".into())
            }),
            Action::CancelPairing { id, label } => self.run_api(move |c| async move {
                c.cancel_pairing(&id).await?;
                Ok(format!("Cancelled pairing request from {label}"))
            }),
        }
    }

    fn save_profiles(&mut self, ok: String) {
        match self.profiles.save() {
            Ok(()) => self.toast(ok, ToastKind::Info),
            Err(e) => self.toast(format!("Could not save hosts: {e:#}"), ToastKind::Error),
        }
    }

    // ---- events -----------------------------------------------------------

    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Connected {
                profiles,
                opts,
                result,
            } => {
                // Connecting may have pinned a new fingerprint.
                self.profiles = *profiles;
                let host = opts.host.clone().unwrap_or_default();
                self.on_connect_result(opts, host, result);
            }
            Msg::Info(Ok(info)) => {
                if !matches!(self.info, Load::Ready(_))
                    && let Some(host) = self.current_host().map(str::to_owned)
                {
                    self.toast(format!("Connected to {host}"), ToastKind::Info);
                    // Remember a username typed into the sign-in dialog.
                    if let Some(user) = self.signed_in_as.take()
                        && let Some(h) = self.profiles.hosts.get_mut(&host)
                        && h.username.is_none()
                    {
                        h.username = Some(user);
                        let _ = self.profiles.save();
                    }
                }
                self.info = Load::Ready(info);
            }
            Msg::Info(Err(e)) => {
                // Connecting doesn't log in, so a wrong password surfaces here.
                if matches!(e, ApiError::Unauthorized)
                    && let Some(host) = self.current_host().map(str::to_owned)
                {
                    let user = self
                        .profiles
                        .hosts
                        .get(&host)
                        .and_then(|h| h.username.clone())
                        .or(self.signed_in_as.take());
                    self.set_failed(host.clone(), e.to_string());
                    self.open_credentials(host, user, None);
                }
                self.info = Load::Failed(e.to_string());
            }
            Msg::Logs(result) => {
                self.logs.in_flight = false;
                match result {
                    Ok(text) => self.logs.set_text(&text),
                    Err(ApiError::Unsupported(_)) => {
                        self.logs.status = super::logs::LogStatus::Unsupported
                    }
                    Err(e) => self.logs.status = super::logs::LogStatus::Failed(e.to_string()),
                }
            }
            Msg::Apps(result) => {
                self.apps.list = match result {
                    Ok(list) => Load::Ready(list),
                    Err(e) => Load::Failed(e.to_string()),
                };
                let n = self.apps.apps().len();
                let sel = self
                    .apps
                    .table
                    .selected()
                    .unwrap_or(0)
                    .min(n.saturating_sub(1));
                self.apps.table.select(Some(sel));
            }
            Msg::Pairing { pending, clients } => self.pairing.set(pending, clients),
            Msg::Done(Ok(text)) => {
                if !text.is_empty() {
                    self.toast(text, ToastKind::Info);
                }
                self.info_at = None;
                self.refresh();
            }
            Msg::Done(Err(e)) => self.toast(e, ToastKind::Error),
        }
    }

    pub fn on_connect_result(
        &mut self,
        opts: ConnectOptions,
        host: String,
        result: Result<Connection, ConnectError>,
    ) {
        match result {
            Ok(conn) => {
                // Connecting doesn't log in; confirm once the first request succeeds.
                self.signed_in_as = opts.username.clone();
                self.set_connected(conn);
            }
            Err(ConnectError::NeedsTrust { url, fingerprint }) => {
                self.set_failed(host.clone(), "certificate not trusted".into());
                self.modal = Some(Modal::Confirm(Confirm {
                    title: "Trust certificate?".into(),
                    body: format!(
                        "{url} presented a certificate that has not been seen before.\n\nSHA-256:\n{fingerprint}\n\nCompare on the host with:\nopenssl x509 -noout -fingerprint -sha256 -in ~/.config/sunshine/credentials/cacert.pem"
                    ),
                    action: Action::Trust { host, fingerprint },
                }));
            }
            Err(ConnectError::NeedsCredentials { username, reason }) => {
                self.set_failed(
                    host.clone(),
                    reason.clone().unwrap_or("credentials required".into()),
                );
                self.open_credentials(host, username, reason);
            }
            Err(ConnectError::Other(e)) => {
                let error = format!("{e:#}");
                if matches!(e.downcast_ref::<ApiError>(), Some(ApiError::Unauthorized)) {
                    self.open_credentials(host.clone(), opts.username.clone(), None);
                }
                self.set_failed(host, error);
            }
        }
    }

    fn open_credentials(&mut self, host: String, username: Option<String>, reason: Option<String>) {
        let form =
            Form::new(
                format!("Sign in to {host}"),
                FormKind::Credentials { host },
                vec![
                    Field::new("Username", Input::new(username.unwrap_or_default())),
                    Field::new("Password", Input::masked()),
                ],
            )
            .note(reason.unwrap_or_else(|| {
                "Tip: set a password_cmd for this host to skip this prompt.".into()
            }));
        self.modal = Some(Modal::Form(form));
    }

    pub fn on_tick(&mut self) {
        if self
            .toast
            .as_ref()
            .is_some_and(|t| t.at.elapsed() > TOAST_TTL)
        {
            self.toast = None;
        }
        if self.client().is_none() || self.modal.is_some() {
            return;
        }
        let due = |at: Option<Instant>, every| at.is_none_or(|t: Instant| t.elapsed() >= every);
        match self.screen {
            Screen::Logs if self.logs.follow && due(self.logs_at, LOGS_REFRESH) => self.load_logs(),
            Screen::Dashboard if due(self.info_at, DASHBOARD_REFRESH) => self.load_info(),
            Screen::Pairing if due(self.pairing.at, PAIRING_REFRESH) => self.load_pairing(),
            _ => {}
        }
    }

    pub fn on_event(&mut self, event: Event) {
        let Event::Key(key) = event else { return };
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if let Some(modal) = self.modal.take() {
            self.on_modal_key(modal, key);
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.modal = Some(Modal::Help),
            KeyCode::Char(c @ '1'..='9') => {
                if let Some(s) = Screen::ALL.get(c as usize - '1' as usize) {
                    self.switch(*s);
                }
            }
            KeyCode::Tab => self.cycle_screen(1),
            KeyCode::BackTab => self.cycle_screen(Screen::ALL.len() - 1),
            KeyCode::Char('R') => {
                self.toast("Refreshing…", ToastKind::Info);
                self.refresh();
            }
            _ => match self.screen {
                Screen::Dashboard => self.dashboard_key(key),
                Screen::Apps => self.apps_key(key),
                Screen::Pairing => self.pairing_key(key),
                Screen::Logs => self.logs_key(key),
                Screen::Hosts => self.hosts_key(key),
            },
        }
    }

    fn cycle_screen(&mut self, by: usize) {
        let i = Screen::ALL
            .iter()
            .position(|s| *s == self.screen)
            .unwrap_or(0);
        self.switch(Screen::ALL[(i + by) % Screen::ALL.len()]);
    }

    fn switch(&mut self, screen: Screen) {
        if self.screen != screen {
            self.screen = screen;
            if screen == Screen::Hosts
                && let Some(cur) = self.current_host()
                && let Some(i) = self.host_names().iter().position(|h| h == cur)
            {
                self.hosts.select(Some(i));
            }
            self.refresh();
        }
    }

    fn on_modal_key(&mut self, modal: Modal, key: KeyEvent) {
        match modal {
            Modal::Help => {}
            Modal::Editor(mut editor) => match editor.handle(key) {
                EditorEvent::Save => self.save_editor(editor),
                EditorEvent::Cancel => {}
                EditorEvent::None => self.modal = Some(Modal::Editor(editor)),
            },
            Modal::Confirm(c) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.run_action(c.action)
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {}
                _ => self.modal = Some(Modal::Confirm(c)),
            },
            Modal::Form(mut form) => match form.handle(key) {
                FormEvent::Cancel => {
                    if matches!(form.kind, FormKind::LogSearch) {
                        self.logs.search = None;
                    }
                }
                FormEvent::Submit => self.submit_form(form),
                FormEvent::None => {
                    if matches!(form.kind, FormKind::LogSearch) {
                        self.logs.search = Some(form.value(0).to_owned()).filter(|s| !s.is_empty());
                    }
                    self.modal = Some(Modal::Form(form));
                }
            },
        }
    }

    fn submit_form(&mut self, form: Form<FormKind>) {
        match form.kind.clone() {
            FormKind::Pair { .. } => self.submit_pair(form),
            FormKind::LogSearch => {
                self.logs.search = Some(form.value(0).to_owned()).filter(|s| !s.is_empty());
                if self.logs.search.is_some() && !self.logs.find(true) && !self.logs.find(false) {
                    self.toast("No matches", ToastKind::Error);
                }
            }
            FormKind::Credentials { host } => {
                let opts = ConnectOptions {
                    host: Some(host),
                    username: Some(form.value(0).to_owned()),
                    password: Some(form.fields[1].input.value.clone()),
                    insecure: self.insecure,
                    ..Default::default()
                };
                self.start_connect(opts);
            }
            FormKind::AddHost => {
                let name = form.value(0).to_owned();
                let url = match profiles::normalize_url(form.value(1)) {
                    Ok(u) => u.as_str().trim_end_matches('/').to_owned(),
                    Err(e) => {
                        self.toast(format!("{e:#}"), ToastKind::Error);
                        self.modal = Some(Modal::Form(form));
                        return;
                    }
                };
                if name.is_empty() || self.profiles.hosts.contains_key(&name) {
                    self.toast("Enter a unique name for the host", ToastKind::Error);
                    self.modal = Some(Modal::Form(form));
                    return;
                }
                let opt = |s: &str| Some(s.to_owned()).filter(|s| !s.is_empty());
                self.profiles.hosts.insert(
                    name.clone(),
                    Host {
                        url,
                        username: opt(form.value(2)),
                        password_cmd: opt(form.value(3)),
                        fingerprint: None,
                    },
                );
                self.save_profiles(format!("Saved '{name}'"));
                if let Some(i) = self.host_names().iter().position(|h| *h == name) {
                    self.hosts.select(Some(i));
                }
                self.connect_to(&name);
            }
        }
    }

    // ---- per-screen keys --------------------------------------------------

    fn dashboard_key(&mut self, key: KeyEvent) {
        if self.client().is_none() {
            if key.code == KeyCode::Enter
                && let Some(h) = self.current_host().map(str::to_owned)
            {
                self.connect_to(&h);
            }
            return;
        }
        match key.code {
            KeyCode::Char('r') => {
                self.modal = Some(Modal::Confirm(Confirm {
                    title: "Restart Sunshine?".into(),
                    body: "Active streams will be interrupted.".into(),
                    action: Action::Restart,
                }))
            }
            KeyCode::Char('x') => {
                self.modal = Some(Modal::Confirm(Confirm {
                    title: "Close the running app?".into(),
                    body: "The current streaming session's app will be terminated.".into(),
                    action: Action::CloseApp,
                }))
            }
            _ => {}
        }
    }

    fn logs_key(&mut self, key: KeyEvent) {
        let page = self.logs.view_height.max(1) as isize;
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.logs.scroll(1),
            KeyCode::Up | KeyCode::Char('k') => self.logs.scroll(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.logs.scroll(page),
            KeyCode::PageUp | KeyCode::Char('b') => self.logs.scroll(-page),
            KeyCode::Home | KeyCode::Char('g') => {
                self.logs.follow = false;
                self.logs.top = 0;
            }
            KeyCode::End | KeyCode::Char('G') => self.logs.follow = true,
            KeyCode::Char('f') => {
                self.logs.follow = !self.logs.follow;
                if !self.logs.follow {
                    self.logs.top = self.logs.first_row(usize::MAX);
                }
            }
            KeyCode::Char('l') => self.logs.cycle_level(),
            KeyCode::Char('/') => {
                let current = self.logs.search.clone().unwrap_or_default();
                self.modal = Some(Modal::Form(Form::new(
                    "Search logs",
                    FormKind::LogSearch,
                    vec![Field::new("Find", Input::new(current))],
                )));
            }
            KeyCode::Char('n') => {
                if !self.logs.find(true) {
                    self.toast("No more matches", ToastKind::Error);
                }
            }
            KeyCode::Char('N') => {
                if !self.logs.find(false) {
                    self.toast("No earlier matches", ToastKind::Error);
                }
            }
            KeyCode::Esc => self.logs.search = None,
            _ => {}
        }
    }

    fn clamp_host_selection(&mut self) {
        let n = self.profiles.hosts.len();
        let i = self.hosts.selected().unwrap_or(0).min(n.saturating_sub(1));
        self.hosts.select(Some(i));
    }

    fn selected_host(&self) -> Option<String> {
        self.hosts
            .selected()
            .and_then(|i| self.host_names().get(i).cloned())
    }

    fn hosts_key(&mut self, key: KeyEvent) {
        let n = self.profiles.hosts.len();
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.hosts.select(Some(
                (self.hosts.selected().unwrap_or(0) + 1).min(n.saturating_sub(1)),
            )),
            KeyCode::Up | KeyCode::Char('k') => self
                .hosts
                .select(Some(self.hosts.selected().unwrap_or(0).saturating_sub(1))),
            KeyCode::Enter => {
                if let Some(h) = self.selected_host() {
                    self.connect_to(&h);
                }
            }
            KeyCode::Char('a') => {
                let form = Form::new(
                    "Add host",
                    FormKind::AddHost,
                    vec![
                        Field::new("Name", Input::default()).hint("e.g. living-room"),
                        Field::new("Address", Input::default())
                            .hint("host, host:port or https://host:port"),
                        Field::new("Username", Input::default()).hint("optional"),
                        Field::new("Password command", Input::default())
                            .hint("optional, e.g. pass show sunshine"),
                    ],
                );
                self.modal = Some(Modal::Form(form));
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if let Some(h) = self.selected_host() {
                    if h == profiles::LOCAL {
                        self.toast("The local host can't be removed", ToastKind::Error);
                        return;
                    }
                    self.modal = Some(Modal::Confirm(Confirm {
                        title: format!("Remove '{h}'?"),
                        body:
                            "Its saved address, username and pinned certificate will be forgotten."
                                .into(),
                        action: Action::RemoveHost(h),
                    }));
                }
            }
            KeyCode::Char('s') => {
                if let Some(h) = self.selected_host() {
                    self.profiles.default = Some(h.clone());
                    self.save_profiles(format!("'{h}' is now the default host"));
                }
            }
            KeyCode::Char('t') => {
                if let Some(h) = self.selected_host() {
                    if let Some(host) = self.profiles.hosts.get_mut(&h) {
                        host.fingerprint = None;
                    }
                    self.save_profiles(format!("Forgot the pinned certificate for '{h}'"));
                    self.connect_to(&h);
                }
            }
            _ => {}
        }
    }
}

async fn fetch_info(client: &SunshineClient) -> Result<Info, ApiError> {
    let (cfg, apps, clients) = tokio::join!(client.config(), client.apps(), client.clients());
    let cfg = cfg?;
    Ok(Info {
        version: cfg.version,
        platform: cfg.platform,
        settings: cfg.values.len(),
        apps: apps.ok().map(|a| a.apps.len()),
        clients: clients.ok().map(|c| c.len()),
        fingerprint: client.seen_fingerprint(),
    })
}
