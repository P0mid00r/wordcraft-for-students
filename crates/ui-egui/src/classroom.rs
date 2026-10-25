use egui::{Align2, Color32, FontId, Id, RichText};
use wordcraft_classroom::{CaptchaKind, CaptchaPrompt, DEFAULT_PORT, Hello, HelloReply, PROTOCOL_VERSION, PollReply, ServerMessage, SessionStatus};

use crate::WordApp;

pub const PAUSED: &str = "classroom access is not active";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Off,
    Connecting,
    Pending,
    Active,
    Denied,
    Disconnected,
}

pub enum Event {
    Connect { url: String, hello: Hello },
    Leave,
    Answer { id: String, answer: String },
    Suspicion { reason: String, detail: String },
}

pub struct Classroom {
    pub available: bool,
    pub phase: Phase,
    pub join_open: bool,
    pub address: String,
    pub port: String,
    pub full_name: String,
    pub hostname: Option<String>,
    pub platform: String,
    pub client_id: String,
    pub session: Option<String>,
    pub cursor: u64,
    pub captcha: Option<CaptchaPrompt>,
    pub captcha_answer: String,
    pub captcha_message: String,
    pub join_error: String,
    pub notice: String,
    url: String,
    hello: Option<Hello>,
    captcha_focus: bool,
    join_focus: bool,
    outbox: Vec<Event>,
}

impl Default for Classroom {
    fn default() -> Self {
        Self {
            available: false,
            phase: Phase::Off,
            join_open: false,
            address: String::new(),
            port: DEFAULT_PORT.to_string(),
            full_name: String::new(),
            hostname: None,
            platform: platform_name(),
            client_id: String::new(),
            session: None,
            cursor: 0,
            captcha: None,
            captcha_answer: String::new(),
            captcha_message: String::new(),
            join_error: String::new(),
            notice: String::new(),
            url: String::new(),
            hello: None,
            captcha_focus: false,
            join_focus: false,
            outbox: Vec::new(),
        }
    }
}

fn platform_name() -> String {
    if cfg!(target_arch = "wasm32") { "web".into() } else { format!("{} {}", std::env::consts::OS, std::env::consts::ARCH) }
}

impl Classroom {
    pub fn open_join(&mut self) {
        if !self.available {
            return;
        }
        self.join_open = true;
        self.join_error.clear();
        self.join_focus = true;
    }

    pub fn submit_join(&mut self) {
        let name = self.full_name.trim();
        if name.is_empty() {
            self.join_error = crate::tl!("Enter your full name.").into();
            return;
        }
        let url = if cfg!(target_arch = "wasm32") {
            String::new()
        } else {
            let address = self.address.trim().trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/');
            if address.is_empty() {
                self.join_error = crate::tl!("Enter the teacher's address.").into();
                return;
            }
            let port = match self.port.trim().parse::<u16>() {
                Ok(0) | Err(_) => {
                    self.join_error = crate::tl!("Enter a port from 1 to 65535.").into();
                    return;
                }
                Ok(port) => port,
            };
            format!("http://{address}:{port}")
        };
        let hello = Hello {
            protocol: PROTOCOL_VERSION,
            client_id: if self.client_id.is_empty() { format!("anon-{}", crate::now_ms() as u64) } else { self.client_id.clone() },
            full_name: name.to_string(),
            hostname: self.hostname.clone(),
            platform: self.platform.clone(),
        };
        self.url = url.clone();
        self.hello = Some(hello.clone());
        self.phase = Phase::Connecting;
        self.join_open = false;
        self.notice.clear();
        self.outbox.push(Event::Connect { url, hello });
    }

    pub fn leave(&mut self) {
        self.phase = Phase::Off;
        self.session = None;
        self.captcha = None;
        self.notice.clear();
        self.join_error.clear();
        self.outbox.push(Event::Leave);
    }

    pub fn reconnect(&mut self) {
        let Some(hello) = self.hello.clone() else {
            self.open_join();
            return;
        };
        self.phase = Phase::Connecting;
        self.captcha = None;
        self.notice.clear();
        self.outbox.push(Event::Connect { url: self.url.clone(), hello });
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.outbox)
    }

    pub fn connected(&mut self, reply: HelloReply) {
        self.session = Some(reply.session);
        self.cursor = reply.cursor;
        self.apply_status(reply.status);
        for message in reply.messages {
            self.message(message);
        }
    }

    pub fn incoming(&mut self, reply: PollReply) {
        self.cursor = reply.cursor;
        self.apply_status(reply.status);
        for message in reply.messages {
            self.message(message);
        }
    }

    pub fn failed(&mut self, message: String) {
        log::warn!("classroom connection failed: {message}");
        self.phase = Phase::Disconnected;
        self.notice = crate::tl!("Could not connect to the teacher's server.").into();
        self.captcha = None;
    }

    fn apply_status(&mut self, status: SessionStatus) {
        match status {
            SessionStatus::Pending => self.phase = Phase::Pending,
            SessionStatus::Active => {
                self.phase = Phase::Active;
                self.captcha = None;
            }
            SessionStatus::Denied => {
                self.phase = Phase::Denied;
                self.captcha = None;
            }
            SessionStatus::Captcha => self.phase = Phase::Active,
            SessionStatus::Paused => self.phase = Phase::Active,
            SessionStatus::Offline => self.phase = Phase::Disconnected,
        }
    }

    fn message(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::Pending => self.phase = Phase::Pending,
            ServerMessage::Approved => {
                self.phase = Phase::Active;
                self.captcha = None;
            }
            ServerMessage::Denied { reason } => {
                self.phase = Phase::Denied;
                self.captcha = None;
                self.notice = reason;
            }
            ServerMessage::Revoked => {
                self.phase = Phase::Denied;
                self.captcha = None;
                self.notice = crate::tl!("Access revoked by the teacher.").into();
            }
            ServerMessage::Captcha { prompt } => {
                self.captcha = Some(prompt);
                self.captcha_answer.clear();
                self.captcha_message.clear();
                self.captcha_focus = true;
            }
            ServerMessage::CaptchaResult { id, ok, message } => {
                if self.captcha.as_ref().is_some_and(|captcha| captcha.id == id) {
                    if ok {
                        self.captcha = None;
                        self.captcha_message.clear();
                        self.phase = Phase::Active;
                    } else {
                        self.captcha_message = message;
                    }
                }
            }
            ServerMessage::Pause { .. } | ServerMessage::Unpause => {}
            ServerMessage::Shutdown => {
                self.phase = Phase::Disconnected;
                self.notice = crate::tl!("The teacher's server stopped.").into();
                self.captcha = None;
            }
        }
    }

    pub fn answer(&mut self) {
        let Some(captcha) = &self.captcha else { return };
        let answer = self.captcha_answer.trim();
        if answer.is_empty() {
            self.captcha_message = crate::tl!("Enter the answer.").into();
            return;
        }
        self.outbox.push(Event::Answer { id: captcha.id.clone(), answer: answer.to_string() });
        self.captcha_message = crate::tl!("Checking…").into();
    }

    pub fn report_suspicion(&mut self, reason: String, detail: String) {
        if self.is_connected() {
            self.outbox.push(Event::Suspicion { reason, detail });
        }
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.phase, Phase::Pending | Phase::Active)
    }

    pub fn blocks_editing(&self) -> bool {
        matches!(self.phase, Phase::Connecting | Phase::Pending | Phase::Denied | Phase::Disconnected) || self.captcha.is_some()
    }

    pub fn status(&self) -> Option<&'static str> {
        match self.phase {
            Phase::Off => None,
            Phase::Connecting => Some(crate::tl!("Connecting…")),
            Phase::Pending => Some(crate::tl!("Waiting for the teacher's approval")),
            Phase::Active => Some(crate::tl!("Class access: connected")),
            Phase::Denied => Some(crate::tl!("Access denied")),
            Phase::Disconnected => Some(crate::tl!("Class access: offline")),
        }
    }
}

pub fn access_button(app: &mut WordApp, ui: &mut egui::Ui) {
    if !app.classroom.available {
        return;
    }
    if let Some(status) = app.classroom.status() {
        ui.label(RichText::new(status).color(crate::theme::Tokens::get(ui.ctx()).text_dim));
    }
    if app.classroom.phase == Phase::Off && ui.button(crate::tl!("Get access to the class")).clicked() {
        let _ = app.run("ui.classroomAccess", serde_json::json!({}));
    }
}

pub fn show(app: &mut WordApp, ctx: &egui::Context) {
    if app.classroom.join_open {
        let mut close = false;
        let mut connect = false;
        let _ = egui::Modal::new(Id::new("wordcraft.classroomJoin")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(crate::tl!("Class access"));
            ui.add_space(6.0);
            if app.classroom.join_focus {
                let focus = if cfg!(target_arch = "wasm32") { Id::new("wordcraft.classroomFullName") } else { Id::new("wordcraft.classroomAddress") };
                ui.memory_mut(|memory| memory.request_focus(focus));
                app.classroom.join_focus = false;
            }
            if !cfg!(target_arch = "wasm32") {
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Teacher address"));
                    ui.add(egui::TextEdit::singleline(&mut app.classroom.address).id(Id::new("wordcraft.classroomAddress")).desired_width(180.0));
                    ui.label(crate::tl!("Port"));
                    ui.add(egui::TextEdit::singleline(&mut app.classroom.port).desired_width(70.0));
                });
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Full name"));
                ui.add(egui::TextEdit::singleline(&mut app.classroom.full_name).id(Id::new("wordcraft.classroomFullName")).desired_width(260.0));
            });
            if !app.classroom.join_error.is_empty() {
                ui.add_space(4.0);
                ui.colored_label(ui.visuals().error_fg_color, &app.classroom.join_error);
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Connect")).clicked() {
                    connect = true;
                }
                if ui.button(crate::tl!("Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if connect {
            app.classroom.submit_join();
        } else if close {
            app.classroom.join_open = false;
        }
        return;
    }

    match app.classroom.phase {
        Phase::Connecting => {
            let mut leave = false;
            let _ = egui::Modal::new(Id::new("wordcraft.classroomConnecting")).show(ctx, |ui| {
                ui.set_max_width(380.0);
                ui.heading(crate::tl!("Connecting…"));
                ui.add_space(8.0);
                ui.spinner();
                ui.add_space(8.0);
                if ui.button(crate::tl!("Cancel")).clicked() {
                    leave = true;
                }
            });
            if leave {
                app.classroom.leave();
                app.input_guard.release();
            }
        }
        Phase::Pending => {
            let mut leave = false;
            let _ = egui::Modal::new(Id::new("wordcraft.classroomPending")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading(crate::tl!("Waiting for the teacher's approval"));
                ui.add_space(8.0);
                ui.label(crate::tl!("The teacher must allow this computer before you can edit."));
                ui.add_space(10.0);
                if ui.button(crate::tl!("Leave")).clicked() {
                    leave = true;
                }
            });
            if leave {
                app.classroom.leave();
                app.input_guard.release();
            }
        }
        Phase::Denied => {
            let mut retry = false;
            let mut close = false;
            let _ = egui::Modal::new(Id::new("wordcraft.classroomDenied")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading(crate::tl!("Access denied"));
                ui.add_space(8.0);
                if app.classroom.notice.is_empty() {
                    ui.label(crate::tl!("The teacher did not allow this computer."));
                } else {
                    ui.label(&app.classroom.notice);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button(crate::tl!("Reconnect")).clicked() {
                        retry = true;
                    }
                    if ui.button(crate::tl!("Close")).clicked() {
                        close = true;
                    }
                });
            });
            if retry {
                app.classroom.open_join();
            } else if close {
                app.classroom.leave();
                app.input_guard.release();
            }
        }
        Phase::Disconnected => {
            let mut retry = false;
            let mut close = false;
            let _ = egui::Modal::new(Id::new("wordcraft.classroomDisconnected")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading(crate::tl!("Connection lost"));
                ui.add_space(8.0);
                if app.classroom.notice.is_empty() {
                    ui.label(crate::tl!("The connection to the teacher was lost."));
                } else {
                    ui.label(&app.classroom.notice);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button(crate::tl!("Reconnect")).clicked() {
                        retry = true;
                    }
                    if ui.button(crate::tl!("Close")).clicked() {
                        close = true;
                    }
                });
            });
            if retry {
                app.classroom.reconnect();
            } else if close {
                app.classroom.leave();
                app.input_guard.release();
            }
        }
        Phase::Off | Phase::Active => {
            if let Some(captcha) = app.classroom.captcha.clone() {
                show_captcha(app, ctx, &captcha);
            }
        }
    }
}

fn show_captcha(app: &mut WordApp, ctx: &egui::Context, captcha: &CaptchaPrompt) {
    let mut submit = false;
    let _ = egui::Modal::new(Id::new("wordcraft.classroomCaptcha")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.heading(crate::tl!("Verification"));
        ui.add_space(6.0);
        ui.label(crate::tl!("Your teacher asks you to prove you are typing."));
        ui.add_space(8.0);
        if captcha.kind == CaptchaKind::Text {
            distorted_code(ui, &captcha.prompt, &captcha.id);
            ui.label(RichText::new(crate::tl!("Type the code shown.")).color(crate::theme::Tokens::get(ui.ctx()).text_dim));
        } else {
            ui.label(RichText::new(&captcha.prompt).font(FontId::monospace(28.0)));
            ui.label(RichText::new(crate::tl!("Answer")).color(crate::theme::Tokens::get(ui.ctx()).text_dim));
        }
        ui.add_space(8.0);
        let id = Id::new("wordcraft.classroomCaptchaAnswer");
        if app.classroom.captcha_focus {
            ui.memory_mut(|memory| memory.request_focus(id));
            app.classroom.captcha_focus = false;
        }
        let response = ui.add(egui::TextEdit::singleline(&mut app.classroom.captcha_answer).id(id).desired_width(240.0));
        if (response.lost_focus() || response.has_focus()) && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            submit = true;
        }
        if !app.classroom.captcha_message.is_empty() {
            ui.add_space(4.0);
            ui.label(&app.classroom.captcha_message);
        }
        ui.add_space(10.0);
        if ui.button(crate::tl!("Submit")).clicked() {
            submit = true;
        }
        if captcha.kind == CaptchaKind::Text {
            ui.add_space(4.0);
            ui.label(RichText::new(crate::tl!("Ask the teacher if you cannot read the code.")).color(crate::theme::Tokens::get(ui.ctx()).text_dim));
        }
    });
    if submit {
        app.classroom.answer();
    }
}

fn distorted_code(ui: &mut egui::Ui, code: &str, seed: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(340.0, 76.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 6.0, Color32::from_gray(246));
    let mut state = seed.bytes().fold(0xcbf29ce484222325u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3));
    let count = code.chars().count().max(1) as f32;
    let step = rect.width() / (count + 1.0);
    for (index, ch) in code.chars().enumerate() {
        let x = rect.left() + step * (index as f32 + 1.0) + (noise(&mut state) - 0.5) * 14.0;
        let y = rect.center().y + (noise(&mut state) - 0.5) * 22.0;
        let color =
            Color32::from_rgb(24 + (noise(&mut state) * 40.0) as u8, 24 + (noise(&mut state) * 40.0) as u8, 24 + (noise(&mut state) * 40.0) as u8);
        painter.text(egui::pos2(x, y), Align2::CENTER_CENTER, ch, FontId::monospace(34.0), color);
    }
    for _ in 0..3 {
        let y = rect.top() + 12.0 + noise(&mut state) * (rect.height() - 24.0);
        let point = |state: &mut u64| egui::pos2(rect.left() + noise(state) * rect.width(), y + (noise(state) - 0.5) * 18.0);
        let a = point(&mut state);
        let b = point(&mut state);
        let c = point(&mut state);
        painter.add(egui::Shape::line(vec![a, b, c], egui::Stroke::new(1.0, Color32::from_gray(120))));
    }
    for _ in 0..25 {
        let x = rect.left() + noise(&mut state) * rect.width();
        let y = rect.top() + noise(&mut state) * rect.height();
        painter.circle_filled(egui::pos2(x, y), 1.0 + noise(&mut state) * 1.5, Color32::from_gray(150));
    }
}

fn noise(state: &mut u64) -> f32 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*state >> 33) as f32) / (u32::MAX as f32)
}
