use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gloo_net::http::Request;
use gloo_timers::future::TimeoutFuture;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;
use wordcraft_classroom::{Ack, CaptchaAnswer, Hello, HelloReply, PollReply, SuspicionReport};
use wordcraft_ui_egui::classroom::Event;

pub enum Incoming {
    Connected(HelloReply),
    Poll(PollReply),
    Failed(String),
}

pub struct Link {
    api: String,
    queue: Rc<RefCell<Vec<(u64, Incoming)>>>,
    generation: Rc<Cell<u64>>,
    session: Rc<RefCell<Option<String>>>,
}

impl Link {
    pub fn new(api: String) -> Self {
        Self { api, queue: Rc::default(), generation: Rc::default(), session: Rc::default() }
    }

    pub fn poll(&mut self) -> Vec<Incoming> {
        let current = self.generation.get();
        let queued = std::mem::take(&mut *self.queue.borrow_mut());
        queued.into_iter().filter_map(|(generation, message)| (generation == current).then_some(message)).collect()
    }

    pub fn send(&mut self, event: Event) {
        match event {
            Event::Connect { hello, .. } => self.connect(hello),
            Event::Leave => {
                self.generation.set(self.generation.get().wrapping_add(1));
                if let Some(session) = self.session.borrow_mut().take() {
                    let api = self.api.clone();
                    spawn_local(async move {
                        let _ = Request::post(&format!("{api}/leave?session={session}")).send().await;
                    });
                }
            }
            Event::Answer { id, answer } => {
                let Some(session) = self.session.borrow().clone() else { return };
                let api = self.api.clone();
                spawn_local(async move {
                    let request = CaptchaAnswer { session, id, answer };
                    if let Err(error) = post::<_, Ack>(&format!("{api}/answer"), &request).await {
                        log::warn!("classroom answer failed: {error}");
                    }
                });
            }
            Event::Suspicion { reason, detail } => {
                let Some(session) = self.session.borrow().clone() else { return };
                let api = self.api.clone();
                spawn_local(async move {
                    let request = SuspicionReport { session, reason, detail };
                    if let Err(error) = post::<_, Ack>(&format!("{api}/suspicion"), &request).await {
                        log::warn!("classroom suspicion failed: {error}");
                    }
                });
            }
        }
    }

    fn connect(&mut self, hello: Hello) {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let api = self.api.clone();
        let queue = self.queue.clone();
        let session = self.session.clone();
        let current = self.generation.clone();
        spawn_local(async move {
            let reply = match post::<_, HelloReply>(&format!("{api}/hello"), &hello).await {
                Ok(reply) => reply,
                Err(error) => {
                    if current.get() == generation {
                        queue.borrow_mut().push((generation, Incoming::Failed(error)));
                    }
                    return;
                }
            };
            let id = reply.session.clone();
            let mut cursor = reply.cursor;
            if current.get() != generation {
                return;
            }
            *session.borrow_mut() = Some(id.clone());
            queue.borrow_mut().push((generation, Incoming::Connected(reply)));
            loop {
                if current.get() != generation {
                    break;
                }
                let url = format!("{api}/poll?session={id}&cursor={cursor}");
                match get::<PollReply>(&url).await {
                    Ok(reply) => {
                        if current.get() != generation {
                            break;
                        }
                        cursor = reply.cursor;
                        queue.borrow_mut().push((generation, Incoming::Poll(reply)));
                    }
                    Err(error) => {
                        if current.get() == generation {
                            queue.borrow_mut().push((generation, Incoming::Failed(error)));
                        }
                        break;
                    }
                }
                TimeoutFuture::new(500).await;
            }
        });
    }
}

async fn post<B: serde::Serialize, R: serde::de::DeserializeOwned>(url: &str, body: &B) -> Result<R, String> {
    let request = Request::post(url).json(body).map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    response.json::<R>().await.map_err(|error| error.to_string())
}

async fn get<R: serde::de::DeserializeOwned>(url: &str) -> Result<R, String> {
    let response = Request::get(url).send().await.map_err(|error| error.to_string())?;
    response.json::<R>().await.map_err(|error| error.to_string())
}

pub fn config() -> Option<String> {
    let global = js_sys::global();
    let value = js_sys::Reflect::get(&global, &JsValue::from_str("__WORDCRAFT_CLASSROOM__")).ok()?;
    if !value.is_object() {
        return None;
    }
    let api = js_sys::Reflect::get(&value, &JsValue::from_str("api")).ok()?;
    api.as_string()
}

pub fn client_id() -> String {
    let storage = web_sys::window().and_then(|window| window.local_storage().ok().flatten());
    if let Some(storage) = &storage
        && let Ok(Some(id)) = storage.get_item("wordcraft.classroomId")
        && !id.is_empty()
    {
        return id;
    }
    let id = format!("web-{:x}-{:x}", js_sys::Date::now() as u64, (js_sys::Math::random() * 1e15) as u64);
    if let Some(storage) = storage {
        let _ = storage.set_item("wordcraft.classroomId", &id);
    }
    id
}
