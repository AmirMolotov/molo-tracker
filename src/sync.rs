use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;
use std::time::Duration;

use rune::TextEdit;
use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::{platform, store::Store};

#[cfg(test)]
mod tests;

const DEFAULT_CONFIG: &str = include_str!("../deploy/sync/sync.example.toml");
const FORMER_DEFAULT_SERVER: &str = "https://scheduler.momentummania.com/tracker-sync";
const FIRST_RETRY_SECONDS: u32 = 30;
const MAX_RETRY_SECONDS: u32 = 300;

#[derive(Deserialize)]
struct Config {
    server_url: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Span {
    pub value: String,
    pub start: i64,
    pub end: i64,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Delta {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub device: String,
    pub id: i64,
    pub app: String,
    pub start: i64,
    pub end: i64,
    pub pages: Vec<Span>,
    pub domains: Vec<Span>,
}

#[derive(Serialize)]
pub(crate) struct Exchange {
    pub cursor: i64,
    pub sessions: Vec<Delta>,
    pub device_name: String,
    pub preferences: PreferencePatch,
    #[serde(skip)]
    pub local_preferences: Preferences,
}

pub(crate) type Preferences = BTreeMap<String, serde_json::Value>;

#[derive(Serialize)]
pub(crate) struct PreferenceChange {
    pub key: String,
    pub expected: serde_json::Value,
    pub value: serde_json::Value,
}

#[derive(Serialize)]
pub(crate) struct PreferencePatch {
    pub seed: bool,
    pub changes: Vec<PreferenceChange>,
}

#[derive(Deserialize)]
struct Download {
    cursor: i64,
    sessions: Vec<Delta>,
    more: bool,
    #[serde(default)]
    devices: BTreeMap<String, String>,
    #[serde(default)]
    preferences: Option<Preferences>,
}

#[derive(Deserialize)]
struct Login {
    token: String,
}

struct HttpError {
    status: Option<u16>,
    message: String,
}

enum ResultEvent {
    Login {
        username: String,
        response: Result<Login, HttpError>,
    },
    Sync {
        sent: Vec<(i64, i64)>,
        local_preferences: Preferences,
        response: Result<Download, HttpError>,
    },
}

pub(crate) struct Client {
    pub server: String,
    pub username: TextEdit,
    pub password: TextEdit,
    pub creating_account: bool,
    pub password_visible: bool,
    pub status: String,
    token: Option<String>,
    task: Option<Receiver<ResultEvent>>,
    next: f64,
    retry_seconds: u32,
}

impl Client {
    pub fn open(path: &Path, store: &mut Store) -> Result<Self, Box<dyn std::error::Error>> {
        let config: Config = match fs::read_to_string(path) {
            Ok(source) => toml::from_str(&source)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::write(path, DEFAULT_CONFIG)?;
                toml::from_str(DEFAULT_CONFIG)?
            }
            Err(error) => return Err(error.into()),
        };
        let server = match server_url(&config.server_url)? {
            server if server == FORMER_DEFAULT_SERVER => {
                let default: Config = toml::from_str(DEFAULT_CONFIG)?;
                fs::write(path, DEFAULT_CONFIG)?;
                server_url(&default.server_url)?
            }
            server => server,
        };
        let profile = store.sync_profile()?;
        let token = if profile.server == server {
            profile
                .secret
                .as_deref()
                .map(platform::unprotect)
                .transpose()?
                .map(String::from_utf8)
                .transpose()?
        } else {
            store.clear_sync_login()?;
            None
        };
        Ok(Self::new(server, profile.username, token))
    }

    pub fn preview() -> Self {
        let config: Config = toml::from_str(DEFAULT_CONFIG).expect("valid default sync config");
        Self::new(config.server_url, String::new(), None)
    }

    fn new(server: String, username: String, token: Option<String>) -> Self {
        Self {
            server,
            username: TextEdit::new(username),
            password: TextEdit::new(""),
            creating_account: false,
            password_visible: false,
            status: String::new(),
            token,
            task: None,
            next: 0.0,
            retry_seconds: FIRST_RETRY_SECONDS,
        }
    }

    pub fn switch_auth_mode(&mut self) {
        self.creating_account = !self.creating_account;
        self.clear_password();
        self.status.clear();
    }

    pub fn clear_password(&mut self) {
        self.password = TextEdit::new("");
        self.password_visible = false;
    }

    pub fn is_signed_in(&self) -> bool {
        self.token.is_some()
    }

    pub fn is_busy(&self) -> bool {
        self.task.is_some()
    }

    pub fn login(&mut self, store: &Store) {
        if self.is_busy() || self.is_signed_in() {
            return;
        }
        let username = self.username.text.trim().to_ascii_lowercase();
        if !(3..=128).contains(&username.len()) {
            self.status = "Enter a username between 3 and 128 characters.".into();
            return;
        }
        if !(12..=256).contains(&self.password.text.chars().count()) {
            self.status = "Enter a password between 12 and 256 characters.".into();
            return;
        }
        let profile = match store.sync_profile() {
            Ok(profile) => profile,
            Err(error) => {
                self.status = format!("Could not read this device’s sync settings. {error}");
                return;
            }
        };
        let password = std::mem::take(&mut self.password.text);
        self.clear_password();
        let create = self.creating_account;
        let server = self.server.clone();
        self.status = if create {
            "Creating your account…"
        } else {
            "Signing in…"
        }
        .into();
        self.start(move || {
            let response = post(
                &server, if create { "register" } else { "login" }, None,
                &serde_json::json!({ "username": username, "password": password, "device": profile.device }),
            );
            ResultEvent::Login { username, response }
        });
    }

    pub fn logout(&mut self, store: &mut Store) -> bool {
        if self.is_busy() {
            return false;
        }
        if let Err(error) = store.clear_sync_login() {
            self.status = format!("Could not sign out. {error}");
            return false;
        }
        if let Some(token) = self.token.take() {
            let server = self.server.clone();
            thread::spawn(move || {
                let _: Result<serde_json::Value, _> =
                    post(&server, "logout", Some(&token), &serde_json::json!({}));
            });
        }
        self.creating_account = false;
        self.status = "Signed out. Activity recorded on this PC is still here.".into();
        true
    }

    pub fn sync_now(&mut self) {
        self.next = 0.0;
    }

    fn schedule_retry(&mut self, clock: f64) {
        self.next = clock + f64::from(self.retry_seconds);
        self.status = format!(
            "{} Retrying in {} seconds.",
            self.status, self.retry_seconds
        );
        self.retry_seconds = (self.retry_seconds * 2).min(MAX_RETRY_SECONDS);
    }

    fn start(&mut self, run: impl FnOnce() -> ResultEvent + Send + 'static) {
        let (sender, receiver) = channel();
        thread::spawn(move || {
            let _ = sender.send(run());
        });
        self.task = Some(receiver);
    }

    pub fn tick(&mut self, store: &mut Store, clock: f64) -> bool {
        if let Some(receiver) = &self.task {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => {
                    self.task = None;
                    self.status = if self.is_signed_in() {
                        "Sync stopped unexpectedly."
                    } else {
                        "Sign-in stopped unexpectedly. Try again."
                    }
                    .into();
                    if self.is_signed_in() {
                        self.schedule_retry(clock);
                    }
                    return false;
                }
            };
            self.task = None;
            match result {
                ResultEvent::Login {
                    username,
                    response: Ok(login),
                } => {
                    let saved = platform::protect(login.token.as_bytes())
                        .map_err(|error| error.to_string())
                        .and_then(|secret| {
                            store
                                .bind_sync(&self.server, &username, &secret)
                                .map_err(|error| error.to_string())
                        });
                    match saved {
                        Ok(()) => {
                            self.token = Some(login.token);
                            self.username = TextEdit::new(&username);
                            self.creating_account = false;
                            self.status = "Connected. Sync will start shortly.".into();
                            self.retry_seconds = FIRST_RETRY_SECONDS;
                            self.sync_now();
                            return true;
                        }
                        Err(error) => self.status = format!("Could not save the sign-in. {error}"),
                    }
                }
                ResultEvent::Login {
                    response: Err(error),
                    ..
                } => self.status = error.message,
                ResultEvent::Sync {
                    sent,
                    local_preferences,
                    response: Ok(reply),
                } => match store.apply_sync(
                    &sent,
                    reply.cursor,
                    &reply.sessions,
                    &reply.devices,
                    reply
                        .preferences
                        .as_ref()
                        .map(|prefs| (&local_preferences, prefs)),
                ) {
                    Ok(changed) => {
                        self.status =
                            format!("Last synced at {}.", chrono::Local::now().format("%H:%M"));
                        self.retry_seconds = FIRST_RETRY_SECONDS;
                        self.next = clock
                            + if reply.more || sent.len() >= 100 {
                                1.0
                            } else {
                                15.0
                            };
                        return changed;
                    }
                    Err(error) => self.status = format!("Could not save the sync results. {error}"),
                },
                ResultEvent::Sync {
                    response: Err(error),
                    ..
                } => {
                    self.status = error.message;
                    if error.status == Some(401) && self.logout(store) {
                        self.status = "Your sign-in expired. Sign in again to resume sync.".into();
                        return true;
                    }
                }
            }
            if self.is_signed_in() {
                self.schedule_retry(clock);
            }
            return false;
        }

        let Some(token) = self.token.clone().filter(|_| clock >= self.next) else {
            return false;
        };
        let batch = store
            .sync_profile()
            .and_then(|profile| store.pending_sync(profile.cursor));
        let mut batch = match batch {
            Ok(batch) => batch,
            Err(error) => {
                self.status = format!("Could not read pending activity. {error}");
                self.schedule_retry(clock);
                return false;
            }
        };
        if let Ok(payload) = serde_json::to_vec(&batch) {
            let mut size = payload.len();
            while size > 2_000_000
                && let Some(session) = batch.sessions.pop()
            {
                size =
                    size.saturating_sub(serde_json::to_vec(&session).map_or(0, |data| data.len()));
            }
        }
        let sent = batch
            .sessions
            .iter()
            .map(|session| (session.id, session.end))
            .collect();
        let server = self.server.clone();
        self.start(move || {
            let local_preferences = std::mem::take(&mut batch.local_preferences);
            ResultEvent::Sync {
                sent,
                local_preferences,
                response: post(&server, "sync", Some(&token), &batch),
            }
        });
        false
    }
}

fn server_url(value: &str) -> Result<String, &'static str> {
    let url = Url::parse(value.trim()).map_err(|_| "Enter a valid server_url in sync.toml.")?;
    let loopback = match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        _ => url.host_str() == Some("localhost"),
    };
    if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Use an HTTPS server_url without credentials, a query, or a fragment in sync.toml.",
        );
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn post<T: serde::de::DeserializeOwned>(
    server: &str,
    path: &str,
    token: Option<&str>,
    request: &impl Serialize,
) -> Result<T, HttpError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .build(),
        )
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(12)))
        .build()
        .into();
    let mut builder = agent
        .post(format!("{server}/{path}"))
        .header("Content-Type", "application/json");
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    let error = |message: &str| HttpError {
        status: None,
        message: message.into(),
    };
    let data =
        serde_json::to_vec(request).map_err(|_| error("Could not prepare the sync request."))?;
    let mut response = builder
        .send(data.as_slice())
        .map_err(|_| error("Could not reach the sync server. Activity is saved on this PC."))?;
    let status = response.status();
    let bytes = response
        .body_mut()
        .with_config()
        .limit(2_500_000)
        .read_to_vec()
        .map_err(|_| error("Could not read the sync server response."))?;
    if !status.is_success() {
        let message = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|body| body.get("error")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("Sync server returned HTTP {}.", status.as_u16()));
        return Err(HttpError {
            status: Some(status.as_u16()),
            message,
        });
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| error("The sync server returned an invalid response."))
}
