use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use wordcraft_classroom::{Ack, CaptchaAnswer, Hello, HelloReply, PollReply, SuspicionReport};
use wordcraft_ui_egui::classroom::Event;

pub enum Incoming {
    Connected(HelloReply),
    Poll(PollReply),
    Failed(String),
}

struct Wire {
    generation: u64,
    message: Incoming,
}

enum Command {
    Answer { id: String, answer: String },
    Suspicion { reason: String, detail: String },
    Stop,
}

pub struct Link {
    incoming: Receiver<Wire>,
    outgoing: Sender<Wire>,
    current: Option<Sender<Command>>,
    generation: u64,
}

impl Default for Link {
    fn default() -> Self {
        let (outgoing, incoming) = channel();
        Self { incoming, outgoing, current: None, generation: 0 }
    }
}

impl Link {
    pub fn send(&mut self, event: Event) {
        match event {
            Event::Connect { url, hello } => self.connect(url, hello),
            Event::Leave => self.stop(),
            Event::Answer { id, answer } => {
                if let Some(command) = &self.current {
                    let _ = command.send(Command::Answer { id, answer });
                }
            }
            Event::Suspicion { reason, detail } => {
                if let Some(command) = &self.current {
                    let _ = command.send(Command::Suspicion { reason, detail });
                }
            }
        }
    }

    pub fn poll(&mut self) -> Vec<Incoming> {
        let mut messages = Vec::new();
        while let Ok(wire) = self.incoming.try_recv() {
            if wire.generation == self.generation {
                messages.push(wire.message);
            }
        }
        messages
    }

    pub fn stop(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(command) = self.current.take() {
            let _ = command.send(Command::Stop);
        }
    }

    fn connect(&mut self, url: String, hello: Hello) {
        self.stop();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let (commands, receiver) = channel();
        let outgoing = self.outgoing.clone();
        std::thread::spawn(move || run(url, hello, receiver, outgoing, generation));
        self.current = Some(commands);
    }
}

fn run(url: String, hello: Hello, commands: Receiver<Command>, outgoing: Sender<Wire>, generation: u64) {
    let send = |message| {
        let _ = outgoing.send(Wire { generation, message });
    };
    let base = url.trim_end_matches('/');
    let reply = match post_json::<Hello, HelloReply>(&format!("{base}/classroom/hello"), &hello) {
        Ok(reply) => reply,
        Err(error) => {
            send(Incoming::Failed(error));
            return;
        }
    };
    let session = reply.session.clone();
    let mut cursor = reply.cursor;
    if outgoing.send(Wire { generation, message: Incoming::Connected(reply) }).is_err() {
        return;
    }
    loop {
        match commands.recv_timeout(Duration::from_millis(400)) {
            Ok(Command::Stop) => {
                let _ = ureq::post(&format!("{base}/classroom/leave?session={session}")).call();
                break;
            }
            Ok(Command::Answer { id, answer }) => {
                let request = CaptchaAnswer { session: session.clone(), id, answer };
                if let Err(error) = post_json::<CaptchaAnswer, Ack>(&format!("{base}/classroom/answer"), &request) {
                    send(Incoming::Failed(error));
                    break;
                }
            }
            Ok(Command::Suspicion { reason, detail }) => {
                let request = SuspicionReport { session: session.clone(), reason, detail };
                let _ = post_json::<SuspicionReport, Ack>(&format!("{base}/classroom/suspicion"), &request);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let poll = format!("{base}/classroom/poll?session={session}&cursor={cursor}");
        match ureq::get(&poll).call() {
            Ok(response) => match response.into_json::<PollReply>() {
                Ok(reply) => {
                    cursor = reply.cursor;
                    if outgoing.send(Wire { generation, message: Incoming::Poll(reply) }).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    send(Incoming::Failed(error.to_string()));
                    break;
                }
            },
            Err(error) => {
                send(Incoming::Failed(error.to_string()));
                break;
            }
        }
    }
}

fn post_json<B: serde::Serialize, R: serde::de::DeserializeOwned>(url: &str, body: &B) -> Result<R, String> {
    let response = ureq::post(url).send_json(body).map_err(|error| error.to_string())?;
    response.into_json::<R>().map_err(|error| error.to_string())
}

pub fn hostname() -> Option<String> {
    hostname::get().ok().map(|name| name.to_string_lossy().into_owned())
}

pub fn client_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        return format!("client-{}", std::process::id());
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
