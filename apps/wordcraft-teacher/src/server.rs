use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Form, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use serde::Deserialize;
use tokio_stream::wrappers::ReceiverStream;
use tower_http::services::ServeDir;
use wordcraft_classroom::{
    Ack, CaptchaAnswer, CaptchaKind, CaptchaPrompt, DEFAULT_ADMIN_PORT, DocumentSnapshot, Hello, HelloReply, PollReply, ServerMessage, SessionStatus,
    SuspicionReport,
};

pub const PAUSE_SECONDS: u64 = 60;
const MAX_SESSIONS: usize = 256;
const MAX_DOCUMENT_BASE64: usize = 12 << 20;
const MAX_DOCUMENT_PNG: usize = 8 << 20;

#[derive(Clone)]
pub struct AppState {
    pub classroom: Arc<Mutex<Classroom>>,
    pub token: Arc<str>,
    pub lan_urls: Arc<Vec<String>>,
    pub port: u16,
    pub web_dir: Option<PathBuf>,
}

#[derive(Default)]
pub struct Classroom {
    pub(crate) sessions: HashMap<String, Session>,
    pub(crate) approved: HashSet<String>,
    pub(crate) events: VecDeque<Event>,
}

pub(crate) struct Session {
    pub(crate) id: String,
    pub(crate) client_id: String,
    pub(crate) full_name: String,
    pub(crate) label: String,
    pub(crate) hostname: Option<String>,
    pub(crate) platform: String,
    pub(crate) address: String,
    pub(crate) status: SessionStatus,
    pub(crate) messages: Vec<ServerMessage>,
    pub(crate) offline: bool,
    pub(crate) last_seen: Instant,
    pub(crate) connected_at: Instant,
    pub(crate) captcha: Option<CaptchaState>,
    pub(crate) suspicion: Option<String>,
    pub(crate) paused_until: Option<Instant>,
    pub(crate) document: Option<DocumentView>,
}

pub(crate) struct DocumentView {
    pub(crate) png: Vec<u8>,
    pub(crate) page: u64,
    pub(crate) pages: u64,
    pub(crate) updated: Instant,
    pub(crate) version: u64,
}

#[derive(Clone)]
pub(crate) struct CaptchaState {
    pub(crate) id: String,
    pub(crate) prompt: CaptchaPrompt,
    pub(crate) answer: String,
    pub(crate) attempts: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventKind {
    Info,
    Suspicion,
    Solved,
}

pub(crate) struct Event {
    pub(crate) at: Instant,
    pub(crate) text: String,
    pub(crate) kind: EventKind,
}

impl Classroom {
    fn event(&mut self, text: String) {
        self.event_kind(text, EventKind::Info);
    }

    fn event_kind(&mut self, text: String, kind: EventKind) {
        self.events.push_front(Event { at: Instant::now(), text, kind });
        while self.events.len() > 50 {
            self.events.pop_back();
        }
    }

    fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    fn prune(&mut self) {
        self.sessions.retain(|_, session| {
            session.last_seen.elapsed() < Duration::from_secs(3600)
                && !(session.offline
                    && session.captcha.is_none()
                    && session.paused_until.is_none()
                    && session.last_seen.elapsed() > Duration::from_secs(120))
        });
    }

    fn join(&mut self, hello: Hello, address: SocketAddr) -> HelloReply {
        let now = Instant::now();
        let client_id = truncate(hello.client_id.trim(), 160);
        let approved = self.approved.contains(&client_id);
        let full_name = truncate(hello.full_name.trim(), 120);
        let hostname = hello.hostname.as_deref().map(|name| truncate(name.trim(), 120)).filter(|name| !name.is_empty());
        let platform = truncate(hello.platform.trim(), 80);
        let address = address.ip().to_string();

        let ids: Vec<String> = self.sessions.iter().filter(|(_, session)| session.client_id == client_id).map(|(id, _)| id.clone()).collect();
        if let Some(chosen) = ids
            .iter()
            .max_by_key(|id| {
                self.sessions
                    .get(*id)
                    .map(|session| (session.captcha.is_some(), session.paused_until.is_some_and(|until| until > now), session.last_seen))
                    .unwrap_or((false, false, now))
            })
            .cloned()
        {
            for id in &ids {
                if id != &chosen {
                    self.sessions.remove(id);
                }
            }
            if let Some(session) = self.sessions.get_mut(&chosen) {
                session.last_seen = now;
                session.offline = false;
                let renamed = session.label != session.full_name;
                session.full_name = full_name;
                session.hostname = hostname;
                session.platform = platform;
                session.address = address;
                if !renamed || session.label.is_empty() {
                    session.label = session.full_name.clone();
                }
                if session.paused_until.is_some_and(|until| until <= now) {
                    session.paused_until = None;
                }
                let status = if session.captcha.is_some() {
                    SessionStatus::Captcha
                } else if session.paused_until.is_some() {
                    SessionStatus::Paused
                } else if session.status == SessionStatus::Denied && !approved {
                    SessionStatus::Denied
                } else if approved {
                    SessionStatus::Active
                } else {
                    SessionStatus::Pending
                };
                session.status = status;
                let messages = match status {
                    SessionStatus::Captcha => {
                        session.captcha.as_ref().map(|captcha| vec![ServerMessage::Captcha { prompt: captcha.prompt.clone() }]).unwrap_or_default()
                    }
                    SessionStatus::Paused => vec![ServerMessage::Pause {
                        seconds: session.paused_until.map(|until| until.saturating_duration_since(now).as_secs().max(1)).unwrap_or(1),
                    }],
                    _ => Vec::new(),
                };
                let cursor = session.messages.len() as u64;
                return HelloReply { session: chosen, status, messages, cursor };
            }
        }

        let mut id = random_hex::<8>();
        while self.sessions.contains_key(&id) {
            id = random_hex::<8>();
        }
        let status = if approved { SessionStatus::Active } else { SessionStatus::Pending };
        let mut messages = Vec::new();
        messages.push(if approved { ServerMessage::Approved } else { ServerMessage::Pending });
        let session = Session {
            id: id.clone(),
            client_id,
            full_name: full_name.clone(),
            label: full_name.clone(),
            hostname,
            platform,
            address: address.clone(),
            status,
            messages,
            offline: false,
            last_seen: now,
            connected_at: now,
            captcha: None,
            suspicion: None,
            paused_until: None,
            document: None,
        };
        self.sessions.insert(id.clone(), session);
        let where_ = format!("{} ({address})", full_name);
        if approved {
            self.event(format!("Подключён уже разрешённый компьютер: {where_}"));
        } else {
            self.event(format!("Запрос доступа: {where_}"));
        }
        let session = self.sessions.get(&id);
        let cursor = session.map_or(0, |session| session.messages.len() as u64);
        HelloReply { session: id, status, messages: session.map(|session| session.messages.clone()).unwrap_or_default(), cursor }
    }

    fn poll(&mut self, session_id: &str, cursor: u64) -> Option<PollReply> {
        let approved = self.sessions.get(session_id).is_some_and(|session| self.approved.contains(&session.client_id));
        let session = self.sessions.get_mut(session_id)?;
        session.last_seen = Instant::now();
        session.offline = false;
        if session.status == SessionStatus::Offline {
            session.status = if approved { SessionStatus::Active } else { SessionStatus::Pending };
        }
        if session.status == SessionStatus::Paused && session.paused_until.is_some_and(|until| Instant::now() >= until) {
            session.status = SessionStatus::Active;
            session.paused_until = None;
            session.messages.push(ServerMessage::Unpause);
        }
        let len = session.messages.len();
        let start = usize::try_from(cursor).unwrap_or(usize::MAX).min(len);
        Some(PollReply { status: session.status, messages: session.messages[start..].to_vec(), cursor: len as u64 })
    }

    fn approve(&mut self, id: &str) {
        let Some(client_id) = self.session(id).map(|session| session.client_id.clone()) else { return };
        self.approved.insert(client_id.clone());
        let mut label = None;
        for session in self.sessions.values_mut().filter(|session| session.client_id == client_id) {
            label = Some(session.label.clone());
            if matches!(session.status, SessionStatus::Pending | SessionStatus::Denied) {
                session.status = SessionStatus::Active;
                session.messages.push(ServerMessage::Approved);
            }
        }
        if let Some(label) = label {
            self.event(format!("{label}: доступ разрешён"));
        }
    }

    fn deny(&mut self, id: &str) {
        let Some(client_id) = self.session(id).map(|session| session.client_id.clone()) else { return };
        self.approved.remove(&client_id);
        let mut label = None;
        for session in self.sessions.values_mut().filter(|session| session.client_id == client_id) {
            label = Some(session.label.clone());
            if session.status != SessionStatus::Denied {
                session.status = SessionStatus::Denied;
                session.captcha = None;
                session.paused_until = None;
                session.messages.push(ServerMessage::Denied { reason: crate::ru::DENIED.into() });
            }
        }
        if let Some(label) = label {
            self.event(format!("{label}: доступ отклонён"));
        }
    }

    fn revoke(&mut self, id: &str) {
        let Some(client_id) = self.session(id).map(|session| session.client_id.clone()) else { return };
        self.approved.remove(&client_id);
        let mut label = None;
        for session in self.sessions.values_mut().filter(|session| session.client_id == client_id) {
            label = Some(session.label.clone());
            session.status = SessionStatus::Denied;
            session.captcha = None;
            session.paused_until = None;
            session.messages.push(ServerMessage::Revoked);
        }
        if let Some(label) = label {
            self.event(format!("{label}: доступ отозван"));
        }
    }

    fn rename(&mut self, id: &str, label: &str) {
        let label = truncate(label.trim(), 80);
        if label.is_empty() {
            return;
        }
        let Some(client_id) = self.session(id).map(|session| session.client_id.clone()) else { return };
        for session in self.sessions.values_mut().filter(|session| session.client_id == client_id) {
            session.label = label.clone();
        }
        self.event(format!("{label}: подключение переименовано"));
    }

    fn request_captcha(&mut self, id: &str) {
        let Some(client_id) = self.session(id).map(|session| session.client_id.clone()) else { return };
        if self.session(id).is_none_or(|session| session.status != SessionStatus::Active) {
            return;
        }
        let (prompt, answer) = make_captcha();
        let mut label = None;
        let mut sent = 0u32;
        for session in self
            .sessions
            .values_mut()
            .filter(|session| session.client_id == client_id && session.status == SessionStatus::Active && session.captcha.is_none())
        {
            label = Some(session.label.clone());
            session.captcha = Some(CaptchaState { id: prompt.id.clone(), prompt: prompt.clone(), answer: answer.clone(), attempts: 0 });
            session.status = SessionStatus::Captcha;
            session.messages.push(ServerMessage::Captcha { prompt: prompt.clone() });
            sent += 1;
        }
        if sent > 0
            && let Some(label) = label
        {
            self.event(format!("{label}: отправлена капча"));
        }
    }

    fn pause(&mut self, id: &str) {
        let Some(client_id) = self.session(id).map(|session| session.client_id.clone()) else { return };
        if self.session(id).is_none_or(|session| session.status != SessionStatus::Active) {
            return;
        }
        let mut label = None;
        let until = Instant::now() + Duration::from_secs(PAUSE_SECONDS);
        for session in self.sessions.values_mut().filter(|session| session.client_id == client_id && session.status == SessionStatus::Active) {
            label = Some(session.label.clone());
            session.status = SessionStatus::Paused;
            session.paused_until = Some(until);
            session.messages.push(ServerMessage::Pause { seconds: PAUSE_SECONDS });
        }
        if let Some(label) = label {
            self.event(format!("{label}: ввод приостановлен на {PAUSE_SECONDS} секунд"));
        }
    }

    fn unpause(&mut self, id: &str) {
        let Some(client_id) = self.session(id).map(|session| session.client_id.clone()) else { return };
        let mut label = None;
        for session in self.sessions.values_mut().filter(|session| session.client_id == client_id && session.status == SessionStatus::Paused) {
            label = Some(session.label.clone());
            session.status = SessionStatus::Active;
            session.paused_until = None;
            session.messages.push(ServerMessage::Unpause);
        }
        if let Some(label) = label {
            self.event(format!("{label}: пауза снята"));
        }
    }

    fn release_captcha(&mut self, id: &str) {
        let Some(client_id) = self.session(id).map(|session| session.client_id.clone()) else { return };
        let mut label = None;
        for session in self.sessions.values_mut().filter(|session| session.client_id == client_id) {
            let Some(captcha) = session.captcha.take() else { continue };
            label = Some(session.label.clone());
            session.status = SessionStatus::Active;
            session.messages.push(ServerMessage::CaptchaResult { id: captcha.id, ok: true, message: crate::ru::RELEASED.into() });
        }
        if let Some(label) = label {
            self.event(format!("{label}: капча снята преподавателем"));
        }
    }

    fn answer(&mut self, session_id: &str, id: &str, answer: &str) -> Ack {
        let Some((client_id, kind, expected, captcha_id, label)) = self.session(session_id).and_then(|session| {
            session
                .captcha
                .as_ref()
                .map(|captcha| (session.client_id.clone(), captcha.prompt.kind, captcha.answer.clone(), captcha.id.clone(), session.label.clone()))
        }) else {
            return Ack { ok: false, message: "no captcha".into() };
        };
        if captcha_id != id {
            return Ack { ok: false, message: "stale captcha".into() };
        }
        let given = answer.trim();
        let correct = if kind == CaptchaKind::Arithmetic {
            given.parse::<i64>().ok() == expected.parse::<i64>().ok()
        } else {
            given.eq_ignore_ascii_case(&expected)
        };
        if correct {
            for session in self.sessions.values_mut().filter(|session| session.client_id == client_id) {
                if session.captcha.as_ref().is_some_and(|captcha| captcha.id == id) {
                    session.captcha = None;
                    session.status = SessionStatus::Active;
                    session.messages.push(ServerMessage::CaptchaResult { id: id.to_string(), ok: true, message: crate::ru::SOLVED.into() });
                }
            }
            self.event_kind(format!("{label}: капча решена, доступ разблокирован"), EventKind::Solved);
            return Ack { ok: true, message: String::new() };
        }
        let mut sent_new = false;
        if let Some(session) = self.sessions.get_mut(session_id)
            && let Some(captcha) = session.captcha.as_mut()
            && captcha.id == id
        {
            captcha.attempts = captcha.attempts.saturating_add(1);
            let attempts = captcha.attempts;
            session.messages.push(ServerMessage::CaptchaResult { id: id.to_string(), ok: false, message: crate::ru::WRONG.into() });
            if attempts >= 3 {
                let (prompt, answer) = make_captcha();
                captcha.id = prompt.id.clone();
                captcha.prompt = prompt.clone();
                captcha.answer = answer;
                captcha.attempts = 0;
                session.messages.push(ServerMessage::Captcha { prompt });
                sent_new = true;
            }
        }
        if sent_new {
            self.event(format!("{label}: три неверных ответа, отправлена новая капча"));
        } else {
            self.event(format!("{label}: неверный ответ на капчу"));
        }
        Ack { ok: false, message: crate::ru::WRONG.into() }
    }

    fn suspicion(&mut self, session_id: &str, reason: &str, detail: &str) {
        let Some(label) = self.session(session_id).map(|session| session.label.clone()) else { return };
        let reason = match reason {
            "burst" => "резкий ввод",
            "fast" => "слишком высокая скорость",
            "regular" => "слишком равномерный ритм",
            other => other,
        };
        let text = format!("{} ({})", truncate(reason.trim(), 120), truncate(detail.trim(), 200));
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.suspicion = Some(text.clone());
        }
        self.event_kind(format!("{label}: подозрение на автоввод — {text}"), EventKind::Suspicion);
    }

    fn leave(&mut self, session_id: &str) {
        let Some(label) = self.session(session_id).map(|session| session.label.clone()) else { return };
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.offline = true;
            session.last_seen = Instant::now();
        }
        self.event(format!("{label}: отключился"));
    }
}

pub fn student_router(state: AppState) -> Router {
    let router = Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/classroom/hello", post(hello))
        .route("/classroom/poll", get(poll))
        .route("/classroom/answer", post(answer))
        .route("/classroom/suspicion", post(suspicion))
        .route("/classroom/leave", post(leave))
        .route("/classroom/document", post(document))
        .layer(DefaultBodyLimit::max(16 << 20))
        .with_state(state.clone());
    match &state.web_dir {
        Some(dir) => router.fallback_service(ServeDir::new(dir)),
        None => router.fallback(get(missing_web)),
    }
}

pub fn admin_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(dashboard))
        .route("/rename", get(rename_page))
        .route("/document", get(document_page))
        .route("/document.png", get(document_png))
        .route("/document.mjpeg", get(document_stream))
        .route("/action", post(action))
        .with_state(state)
}

async fn index(State(state): State<AppState>) -> impl IntoResponse {
    let Some(dir) = &state.web_dir else {
        return (StatusCode::SERVICE_UNAVAILABLE, "web assets not found").into_response();
    };
    match tokio::fs::read_to_string(dir.join("index.html")).await {
        Ok(html) => {
            let config = r#"<script>window.__WORDCRAFT_CLASSROOM__={"api":"/classroom"};</script>"#;
            let html = if html.contains("</head>") { html.replacen("</head>", &format!("{config}</head>"), 1) } else { format!("{html}{config}") };
            ([(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"), (axum::http::header::CACHE_CONTROL, "no-store")], html).into_response()
        }
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, format!("cannot read index.html: {error}")).into_response(),
    }
}

async fn missing_web() -> impl IntoResponse {
    (StatusCode::SERVICE_UNAVAILABLE, "WordCraft Teacher: the web folder is missing")
}

async fn hello(
    State(state): State<AppState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Json(hello): Json<Hello>,
) -> Result<Json<HelloReply>, (StatusCode, Json<Ack>)> {
    if hello.protocol != wordcraft_classroom::PROTOCOL_VERSION {
        return Err(api_error(StatusCode::BAD_REQUEST, crate::ru::PROTOCOL));
    }
    if hello.full_name.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, crate::ru::NAME));
    }
    if hello.client_id.trim().is_empty() {
        return Err(api_error(StatusCode::BAD_REQUEST, "missing client id"));
    }
    let mut classroom = state.classroom.lock().map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, crate::ru::STATE))?;
    classroom.prune();
    if classroom.sessions.len() >= MAX_SESSIONS && !classroom.sessions.values().any(|session| session.client_id == hello.client_id) {
        return Err(api_error(StatusCode::TOO_MANY_REQUESTS, "too many connections"));
    }
    Ok(Json(classroom.join(hello, address)))
}

#[derive(Deserialize)]
struct PollQuery {
    session: String,
    #[serde(default)]
    cursor: u64,
}

async fn poll(State(state): State<AppState>, Query(query): Query<PollQuery>) -> Result<Json<PollReply>, (StatusCode, Json<Ack>)> {
    let mut classroom = state.classroom.lock().map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, crate::ru::STATE))?;
    classroom.poll(&query.session, query.cursor).map(Json).ok_or_else(|| api_error(StatusCode::NOT_FOUND, crate::ru::SESSION))
}

async fn answer(State(state): State<AppState>, Json(answer): Json<CaptchaAnswer>) -> Json<Ack> {
    match state.classroom.lock() {
        Ok(mut classroom) => Json(classroom.answer(&answer.session, &answer.id, &answer.answer)),
        Err(_) => Json(Ack { ok: false, message: crate::ru::STATE.into() }),
    }
}

async fn suspicion(State(state): State<AppState>, Json(report): Json<SuspicionReport>) -> Json<Ack> {
    match state.classroom.lock() {
        Ok(mut classroom) => {
            classroom.suspicion(&report.session, &report.reason, &report.detail);
            Json(Ack { ok: true, message: String::new() })
        }
        Err(_) => Json(Ack { ok: false, message: crate::ru::STATE.into() }),
    }
}

#[derive(Deserialize)]
struct SessionQuery {
    session: String,
}

async fn leave(State(state): State<AppState>, Query(query): Query<SessionQuery>) -> Json<Ack> {
    match state.classroom.lock() {
        Ok(mut classroom) => {
            classroom.leave(&query.session);
            Json(Ack { ok: true, message: String::new() })
        }
        Err(_) => Json(Ack { ok: false, message: crate::ru::STATE.into() }),
    }
}

async fn document(State(state): State<AppState>, Json(snapshot): Json<DocumentSnapshot>) -> Json<Ack> {
    if snapshot.png.len() > MAX_DOCUMENT_BASE64 {
        return Json(Ack { ok: false, message: "document too large".into() });
    }
    let png = match base64::engine::general_purpose::STANDARD.decode(snapshot.png.as_bytes()) {
        Ok(png) if png.len() <= MAX_DOCUMENT_PNG && valid_document_png(&png) => png,
        _ => return Json(Ack { ok: false, message: "invalid document image".into() }),
    };
    match state.classroom.lock() {
        Ok(mut classroom) => {
            let Some(session) = classroom.sessions.get_mut(&snapshot.session) else {
                return Json(Ack { ok: false, message: crate::ru::SESSION.into() });
            };
            if !matches!(session.status, SessionStatus::Active | SessionStatus::Captcha | SessionStatus::Paused) {
                return Json(Ack { ok: false, message: "classroom is not active".into() });
            }
            let version = session.document.as_ref().map_or(1, |document| document.version.wrapping_add(1));
            session.document = Some(DocumentView {
                png,
                page: snapshot.page.max(1),
                pages: snapshot.pages.max(snapshot.page.max(1)),
                updated: Instant::now(),
                version,
            });
            Json(Ack { ok: true, message: String::new() })
        }
        Err(_) => Json(Ack { ok: false, message: crate::ru::STATE.into() }),
    }
}

fn valid_document_png(png: &[u8]) -> bool {
    if png.len() < 24 || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return false;
    }
    let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    width > 0 && height > 0 && width <= 3000 && height <= 5000
}

fn api_error(status: StatusCode, message: &str) -> (StatusCode, Json<Ack>) {
    (status, Json(Ack { ok: false, message: message.into() }))
}

#[derive(Deserialize)]
struct TokenQuery {
    token: String,
}

#[derive(Deserialize)]
struct RenameQuery {
    token: String,
    session: String,
}

#[derive(Deserialize)]
struct ActionForm {
    token: String,
    op: String,
    session: String,
    #[serde(default)]
    label: String,
}

async fn dashboard(State(state): State<AppState>, Query(query): Query<TokenQuery>) -> Result<Html<String>, StatusCode> {
    if query.token != *state.token {
        return Err(StatusCode::FORBIDDEN);
    }
    let classroom = state.classroom.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Html(crate::dashboard::page(&state, &classroom)))
}

async fn rename_page(State(state): State<AppState>, Query(query): Query<RenameQuery>) -> Result<Html<String>, StatusCode> {
    if query.token != *state.token {
        return Err(StatusCode::FORBIDDEN);
    }
    let classroom = state.classroom.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let session = classroom.session(&query.session).ok_or(StatusCode::NOT_FOUND)?;
    Ok(Html(crate::dashboard::rename_page(&state, session)))
}

async fn document_page(State(state): State<AppState>, Query(query): Query<RenameQuery>) -> Result<Html<String>, StatusCode> {
    if query.token != *state.token {
        return Err(StatusCode::FORBIDDEN);
    }
    let classroom = state.classroom.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let session = classroom.session(&query.session).ok_or(StatusCode::NOT_FOUND)?;
    Ok(Html(crate::dashboard::document_page(&state, session)))
}

async fn document_stream(State(state): State<AppState>, Query(query): Query<RenameQuery>) -> Result<Response, StatusCode> {
    if query.token != *state.token {
        return Err(StatusCode::FORBIDDEN);
    }
    match state.classroom.lock() {
        Ok(classroom) if classroom.session(&query.session).is_some() => {}
        Ok(_) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, Infallible>>(1);
    let state = state.clone();
    let session_id = query.session;
    tokio::spawn(async move {
        let mut last_version = 0u64;
        loop {
            if sender.is_closed() {
                break;
            }
            let next = match state.classroom.lock() {
                Ok(classroom) => {
                    let Some(session) = classroom.session(&session_id) else { break };
                    session
                        .document
                        .as_ref()
                        .filter(|document| document.version != last_version)
                        .map(|document| (document.version, document.png.clone()))
                }
                Err(_) => break,
            };
            if let Some((version, png)) = next {
                if sender.send(Ok(Bytes::from(multipart_frame(&png)))).await.is_err() {
                    break;
                }
                last_version = version;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });
    Response::builder()
        .header(header::CONTENT_TYPE, "multipart/x-mixed-replace; boundary=frame")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(ReceiverStream::new(receiver)))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn multipart_frame(png: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(png.len() + 96);
    frame.extend_from_slice(b"--frame\r\nContent-Type: image/png\r\nContent-Length: ");
    frame.extend_from_slice(png.len().to_string().as_bytes());
    frame.extend_from_slice(b"\r\n\r\n");
    frame.extend_from_slice(png);
    frame.extend_from_slice(b"\r\n");
    frame
}

async fn document_png(State(state): State<AppState>, Query(query): Query<RenameQuery>) -> Result<impl IntoResponse, StatusCode> {
    if query.token != *state.token {
        return Err(StatusCode::FORBIDDEN);
    }
    let classroom = state.classroom.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let session = classroom.session(&query.session).ok_or(StatusCode::NOT_FOUND)?;
    let document = session.document.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    Ok(([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "no-store")], document.png.clone()))
}

async fn action(State(state): State<AppState>, Form(form): Form<ActionForm>) -> Result<Redirect, StatusCode> {
    if form.token != *state.token {
        return Err(StatusCode::FORBIDDEN);
    }
    {
        let mut classroom = state.classroom.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        match form.op.as_str() {
            "approve" => classroom.approve(&form.session),
            "deny" => classroom.deny(&form.session),
            "revoke" => classroom.revoke(&form.session),
            "rename" => classroom.rename(&form.session, &form.label),
            "captcha" => classroom.request_captcha(&form.session),
            "pause" => classroom.pause(&form.session),
            "unpause" => classroom.unpause(&form.session),
            "release" => classroom.release_captcha(&form.session),
            _ => {}
        }
    }
    Ok(Redirect::to(&format!("/?token={}", state.token)))
}

pub fn make_captcha() -> (CaptchaPrompt, String) {
    let bytes = random_bytes::<8>();
    let id = hex(&bytes);
    if bytes[0].is_multiple_of(2) {
        let alphabet = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
        let mut code = String::new();
        for byte in random_bytes::<5>() {
            let index = usize::from(byte) % alphabet.len();
            code.push(char::from(alphabet[index]));
        }
        (CaptchaPrompt { id, kind: CaptchaKind::Text, prompt: code.clone() }, code)
    } else {
        let random = random_bytes::<2>();
        let a = 2 + u32::from(random[0]) % 19;
        let b = 2 + u32::from(random[1]) % 19;
        if random[0].is_multiple_of(2) {
            (CaptchaPrompt { id, kind: CaptchaKind::Arithmetic, prompt: format!("{a} + {b} = ?") }, (a + b).to_string())
        } else {
            let (large, small) = if a >= b { (a, b) } else { (b, a) };
            (CaptchaPrompt { id, kind: CaptchaKind::Arithmetic, prompt: format!("{large} − {small} = ?") }, (large - small).to_string())
        }
    }
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn random_hex<const N: usize>() -> String {
    hex(&random_bytes::<N>())
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    if getrandom::fill(&mut bytes).is_err() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0u128, |duration| duration.as_nanos());
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (now >> ((index % 16) * 8)) as u8 ^ (index as u8).wrapping_mul(31);
        }
    }
    bytes
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn random_token() -> String {
    hex(&random_bytes::<32>())
}

pub fn admin_url(port: u16, token: &str) -> String {
    format!("http://127.0.0.1:{port}/?token={token}")
}

pub fn default_admin_port() -> u16 {
    DEFAULT_ADMIN_PORT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_images_must_be_small_pngs() {
        let mut png = vec![0u8; 24];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[16..20].copy_from_slice(&900u32.to_be_bytes());
        png[20..24].copy_from_slice(&1200u32.to_be_bytes());
        assert!(valid_document_png(&png));
        png[16..20].copy_from_slice(&9000u32.to_be_bytes());
        assert!(!valid_document_png(&png));
        assert!(!valid_document_png(b"not a png"));
    }

    #[test]
    fn a_captcha_has_an_answer_and_a_prompt() {
        let (prompt, answer) = make_captcha();
        assert!(!prompt.prompt.is_empty());
        assert!(!answer.is_empty());
        assert_eq!(prompt.kind == CaptchaKind::Text, prompt.prompt.len() == 5);
    }

    #[test]
    fn approving_and_revoking_remember_the_client() {
        let mut classroom = Classroom::default();
        let hello = Hello {
            protocol: wordcraft_classroom::PROTOCOL_VERSION,
            client_id: "client".into(),
            full_name: "Иван".into(),
            hostname: Some("PC-1".into()),
            platform: "windows".into(),
        };
        let reply = classroom.join(hello.clone(), SocketAddr::from(([10, 0, 0, 2], 5000)));
        assert_eq!(reply.status, SessionStatus::Pending);
        classroom.approve(&reply.session);
        assert!(classroom.approved.contains("client"));
        classroom.revoke(&reply.session);
        assert!(!classroom.approved.contains("client"));
        let again = classroom.join(hello, SocketAddr::from(([10, 0, 0, 2], 5001)));
        assert_eq!(again.status, SessionStatus::Denied);
        assert_eq!(again.session, reply.session);
        classroom.approve(&again.session);
        assert_eq!(classroom.session(&again.session).map(|session| session.status), Some(SessionStatus::Active));
    }

    #[test]
    fn a_reload_reuses_the_same_session() {
        let mut classroom = Classroom::default();
        let hello = Hello {
            protocol: wordcraft_classroom::PROTOCOL_VERSION,
            client_id: "client".into(),
            full_name: "Иван".into(),
            hostname: None,
            platform: "linux".into(),
        };
        let first = classroom.join(hello.clone(), SocketAddr::from(([10, 0, 0, 2], 5000)));
        let second = classroom.join(hello, SocketAddr::from(([10, 0, 0, 2], 5001)));
        assert_eq!(second.session, first.session);
        assert_eq!(classroom.sessions.len(), 1);
        assert_eq!(classroom.events.iter().filter(|event| event.text.starts_with("Запрос доступа")).count(), 1);
    }

    #[test]
    fn denying_removes_approval_and_a_reload_stays_denied() {
        let mut classroom = Classroom::default();
        let hello = Hello {
            protocol: wordcraft_classroom::PROTOCOL_VERSION,
            client_id: "client".into(),
            full_name: "Иван".into(),
            hostname: None,
            platform: "linux".into(),
        };
        let first = classroom.join(hello.clone(), SocketAddr::from(([10, 0, 0, 2], 5000)));
        classroom.approve(&first.session);
        classroom.deny(&first.session);
        assert!(!classroom.approved.contains("client"));
        let again = classroom.join(hello, SocketAddr::from(([10, 0, 0, 2], 5001)));
        assert_eq!(again.status, SessionStatus::Denied);
        assert_eq!(again.session, first.session);
        classroom.approve(&again.session);
        assert_eq!(classroom.session(&again.session).map(|session| session.status), Some(SessionStatus::Active));
    }

    #[test]
    fn a_captcha_covers_every_tab_of_one_computer() {
        let mut classroom = Classroom::default();
        let hello = Hello {
            protocol: wordcraft_classroom::PROTOCOL_VERSION,
            client_id: "client".into(),
            full_name: "Иван".into(),
            hostname: None,
            platform: "linux".into(),
        };
        let first = classroom.join(hello.clone(), SocketAddr::from(([10, 0, 0, 2], 5000)));
        let second = classroom.join(hello, SocketAddr::from(([10, 0, 0, 2], 5001)));
        classroom.approve(&first.session);
        assert_eq!(classroom.session(&second.session).map(|session| session.status), Some(SessionStatus::Active));
        classroom.request_captcha(&first.session);
        assert_eq!(classroom.session(&second.session).map(|session| session.status), Some(SessionStatus::Captcha));
        let (id, answer) = classroom
            .session(&first.session)
            .and_then(|session| session.captcha.as_ref())
            .map(|captcha| (captcha.prompt.id.clone(), captcha.answer.clone()))
            .unwrap();
        assert!(classroom.answer(&second.session, &id, &answer).ok);
        assert_eq!(classroom.session(&first.session).map(|session| session.status), Some(SessionStatus::Active));
        assert_eq!(classroom.session(&second.session).map(|session| session.status), Some(SessionStatus::Active));
    }

    #[test]
    fn reconnecting_does_not_clear_a_captcha() {
        let mut classroom = Classroom::default();
        let hello = Hello {
            protocol: wordcraft_classroom::PROTOCOL_VERSION,
            client_id: "client".into(),
            full_name: "Иван".into(),
            hostname: None,
            platform: "linux".into(),
        };
        let first = classroom.join(hello.clone(), SocketAddr::from(([10, 0, 0, 2], 5000)));
        classroom.approve(&first.session);
        classroom.request_captcha(&first.session);
        let prompt = classroom.session(&first.session).and_then(|session| session.captcha.as_ref()).map(|captcha| captcha.prompt.clone()).unwrap();
        let second = classroom.join(hello, SocketAddr::from(([10, 0, 0, 2], 5001)));
        assert_eq!(second.status, SessionStatus::Captcha);
        assert_eq!(second.session, first.session);
        assert!(second.messages.iter().any(|message| matches!(message, ServerMessage::Captcha { prompt: carried } if carried.id == prompt.id)));
        assert_eq!(classroom.sessions.len(), 1);
    }

    #[test]
    fn closing_and_reopening_keeps_a_captcha() {
        let mut classroom = Classroom::default();
        let hello = Hello {
            protocol: wordcraft_classroom::PROTOCOL_VERSION,
            client_id: "client".into(),
            full_name: "Иван".into(),
            hostname: None,
            platform: "linux".into(),
        };
        let first = classroom.join(hello.clone(), SocketAddr::from(([10, 0, 0, 2], 5000)));
        classroom.approve(&first.session);
        classroom.request_captcha(&first.session);
        classroom.leave(&first.session);
        let again = classroom.join(hello, SocketAddr::from(([10, 0, 0, 2], 5001)));
        assert_eq!(again.status, SessionStatus::Captcha);
        assert_eq!(again.session, first.session);
    }

    #[test]
    fn a_solved_captcha_is_a_solved_event_and_pause_round_trips() {
        let mut classroom = Classroom::default();
        let hello = Hello {
            protocol: wordcraft_classroom::PROTOCOL_VERSION,
            client_id: "client".into(),
            full_name: "Иван".into(),
            hostname: None,
            platform: "linux".into(),
        };
        let reply = classroom.join(hello, SocketAddr::from(([10, 0, 0, 2], 5000)));
        classroom.approve(&reply.session);
        classroom.request_captcha(&reply.session);
        let (id, answer) = classroom
            .session(&reply.session)
            .and_then(|session| session.captcha.as_ref())
            .map(|captcha| (captcha.prompt.id.clone(), captcha.answer.clone()))
            .unwrap();
        assert!(classroom.answer(&reply.session, &id, &answer).ok);
        assert!(classroom.events.front().is_some_and(|event| event.kind == EventKind::Solved));
        classroom.pause(&reply.session);
        let poll = classroom.poll(&reply.session, 0).unwrap();
        assert!(poll.messages.iter().any(|message| matches!(message, ServerMessage::Pause { seconds: 60 })));
        classroom.unpause(&reply.session);
        assert_eq!(classroom.session(&reply.session).map(|session| session.status), Some(SessionStatus::Active));
    }

    #[test]
    fn a_correct_captcha_unblocks_and_an_incorrect_one_does_not() {
        let mut classroom = Classroom::default();
        let hello = Hello {
            protocol: wordcraft_classroom::PROTOCOL_VERSION,
            client_id: "client".into(),
            full_name: "Иван".into(),
            hostname: None,
            platform: "linux".into(),
        };
        let reply = classroom.join(hello, SocketAddr::from(([10, 0, 0, 2], 5000)));
        classroom.approve(&reply.session);
        classroom.request_captcha(&reply.session);
        let prompt = classroom.session(&reply.session).and_then(|session| session.captcha.as_ref()).map(|captcha| captcha.prompt.clone()).unwrap();
        let wrong = classroom.answer(&reply.session, &prompt.id, "definitely wrong");
        assert!(!wrong.ok);
        assert_eq!(classroom.session(&reply.session).map(|session| session.status), Some(SessionStatus::Captcha));
        let answer = classroom.session(&reply.session).and_then(|session| session.captcha.as_ref()).map(|captcha| captcha.answer.clone()).unwrap();
        let right = classroom.answer(&reply.session, &prompt.id, &answer);
        assert!(right.ok);
        assert_eq!(classroom.session(&reply.session).map(|session| session.status), Some(SessionStatus::Active));
    }
}
