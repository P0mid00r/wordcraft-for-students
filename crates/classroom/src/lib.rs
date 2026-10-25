use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 8770;
pub const DEFAULT_ADMIN_PORT: u16 = 8771;
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Pending,
    Active,
    Denied,
    Captcha,
    Paused,
    Offline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptchaKind {
    Text,
    Arithmetic,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptchaPrompt {
    pub id: String,
    pub kind: CaptchaKind,
    pub prompt: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Pending,
    Approved,
    Denied { reason: String },
    Revoked,
    Captcha { prompt: CaptchaPrompt },
    CaptchaResult { id: String, ok: bool, message: String },
    Pause { seconds: u64 },
    Unpause,
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    pub client_id: String,
    pub full_name: String,
    pub hostname: Option<String>,
    pub platform: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloReply {
    pub session: String,
    pub status: SessionStatus,
    pub messages: Vec<ServerMessage>,
    pub cursor: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollReply {
    pub status: SessionStatus,
    pub messages: Vec<ServerMessage>,
    pub cursor: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptchaAnswer {
    pub session: String,
    pub id: String,
    pub answer: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuspicionReport {
    pub session: String,
    pub reason: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ack {
    pub ok: bool,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_through_json() {
        let message = ServerMessage::Captcha { prompt: CaptchaPrompt { id: "c1".into(), kind: CaptchaKind::Arithmetic, prompt: "2 + 3".into() } };
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(serde_json::from_str::<ServerMessage>(&json).unwrap(), message);
    }

    #[test]
    fn statuses_are_snake_case() {
        assert_eq!(serde_json::to_string(&SessionStatus::Pending).unwrap(), "\"pending\"");
        assert_eq!(serde_json::to_string(&CaptchaKind::Text).unwrap(), "\"text\"");
    }
}
