use std::fmt::Write as _;
use std::time::Instant;

use wordcraft_classroom::SessionStatus;

use crate::server::{AppState, Classroom, EventKind, PAUSE_SECONDS, Session};

pub fn page(state: &AppState, classroom: &Classroom) -> String {
    let mut html = String::new();
    head(&mut html, true, &state.token);
    let _ = write!(html, "<body><main><header><h1>WordCraft Teacher</h1><p class=\"sub\">Локальный сервер для класса</p></header>");
    if state.web_dir.is_none() {
        let _ = write!(
            html,
            "<div class=\"bad\">Папка <code>web</code> не найдена. Положите её рядом с программой, иначе ученики не смогут загрузить WordCraft.</div>"
        );
    }
    let _ = write!(html, "<section><h2>Для учеников</h2><p>Откройте на компьютере ученика:</p><div class=\"urls\">");
    if state.lan_urls.is_empty() {
        let _ = write!(html, "<p class=\"bad\">Не удалось определить адрес в локальной сети.</p>");
    } else {
        for url in state.lan_urls.iter() {
            let _ = write!(html, "<code>{}</code>", escape(url));
        }
    }
    let _ = write!(html, "</div><p class=\"hint\">Порт {} · панель обновляется каждые 2 секунды</p></section>", state.port);

    if let Some(event) = classroom.events.front()
        && event.at.elapsed().as_secs() < 15
    {
        let class = match event.kind {
            EventKind::Info => "info",
            EventKind::Suspicion => "bad",
            EventKind::Solved => "good",
        };
        let _ = write!(html, "<div class=\"notice {class}\">{}</div>", escape(&event.text));
    }

    let now = Instant::now();
    let pending: Vec<&Session> = classroom.sessions.values().filter(|session| session.status == SessionStatus::Pending).collect();
    let blocked: Vec<&Session> = classroom.sessions.values().filter(|session| session.status == SessionStatus::Denied).collect();
    let active: Vec<&Session> = classroom
        .sessions
        .values()
        .filter(|session| matches!(session.status, SessionStatus::Active | SessionStatus::Captcha | SessionStatus::Paused | SessionStatus::Offline))
        .collect();

    let _ = write!(html, "<section><h2>Запросы на доступ</h2>");
    if pending.is_empty() {
        let _ = write!(html, "<p class=\"empty\">Новых запросов нет.</p>");
    }
    for session in pending {
        session_card(&mut html, state, session, now, true);
    }
    let _ = write!(html, "</section>");

    let _ = write!(html, "<section><h2>Подключённые компьютеры</h2>");
    if active.is_empty() {
        let _ = write!(html, "<p class=\"empty\">Пока никто не подключился.</p>");
    }
    for session in active {
        session_card(&mut html, state, session, now, false);
    }
    if !blocked.is_empty() {
        let _ = write!(html, "<h3>Доступ отклонён</h3>");
        for session in blocked {
            session_card(&mut html, state, session, now, false);
        }
    }
    let _ = write!(html, "</section>");

    let _ = write!(html, "<section><h2>События</h2>");
    if classroom.events.is_empty() {
        let _ = write!(html, "<p class=\"empty\">Событий пока нет.</p>");
    } else {
        let _ = write!(html, "<div class=\"events\">");
        for event in classroom.events.iter().take(20) {
            let _ = write!(html, "<div class=\"event\"><span>{}</span>{}</div>", ago(event.at, now), escape(&event.text));
        }
        let _ = write!(html, "</div>");
    }
    let _ = write!(html, "</section>");
    let _ = write!(html, "<footer><a href=\"/?token={}\">Обновить сейчас</a></footer></main></body></html>", escape(&state.token));
    html
}

fn session_card(html: &mut String, state: &AppState, session: &Session, now: Instant, pending: bool) {
    let online = session.last_seen.elapsed().as_secs() < 20 && session.status != SessionStatus::Offline;
    let status = status_text(session);
    let _ = write!(html, "<article class=\"card\"><div class=\"who\"><strong>{}</strong>", escape(&session.label));
    if session.label != session.full_name {
        let _ = write!(html, "<span class=\"muted\"> · {}</span>", escape(&session.full_name));
    }
    let _ = write!(html, "<span class=\"status {}\">{}</span></div>", if online { "on" } else { "off" }, escape(&status));
    let _ = write!(html, "<div class=\"meta\">");
    if let Some(hostname) = &session.hostname {
        let _ = write!(html, "{} · ", escape(hostname));
    }
    let _ = write!(
        html,
        "{} · {} · {} · подключён {}",
        escape(&session.platform),
        escape(&session.address),
        ago(session.connected_at, now),
        escape(&session.id)
    );
    let _ = write!(html, "</div>");
    if let Some(suspicion) = &session.suspicion {
        let _ = write!(html, "<div class=\"suspect\">Подозрение на автоввод: {}</div>", escape(suspicion));
    }
    if let Some(captcha) = &session.captcha {
        let _ = write!(html, "<div class=\"captcha\">Капча ожидает ответа (попыток: {})</div>", captcha.attempts);
    }
    if session.status == SessionStatus::Paused
        && let Some(until) = session.paused_until
    {
        let left = until.saturating_duration_since(now).as_secs().saturating_add(1);
        let _ = write!(html, "<div class=\"captcha\">Ввод приостановлен, ещё {left} с</div>");
    }
    let _ = write!(html, "<div class=\"actions\">");
    if session.status == SessionStatus::Pending {
        action(html, state, session, "approve", "Разрешить");
        action(html, state, session, "deny", "Отклонить");
    } else if session.status == SessionStatus::Denied {
        action(html, state, session, "approve", "Разрешить снова");
    } else {
        if session.status == SessionStatus::Active && online {
            action(html, state, session, "captcha", "Капча");
            action(html, state, session, "pause", &format!("Пауза {PAUSE_SECONDS} с"));
        }
        if session.status == SessionStatus::Captcha {
            action(html, state, session, "release", "Снять капчу");
        }
        if session.status == SessionStatus::Paused {
            action(html, state, session, "unpause", "Снять паузу");
        }
        let _ = write!(html, "<a class=\"button\" href=\"/rename?token={}&session={}\">Переименовать</a>", escape(&state.token), escape(&session.id));
        action(html, state, session, "revoke", "Отозвать доступ");
    }
    if pending {
        let _ = write!(html, "<a class=\"button\" href=\"/rename?token={}&session={}\">Переименовать</a>", escape(&state.token), escape(&session.id));
    }
    let _ = write!(html, "</div></article>");
}

fn status_text(session: &Session) -> String {
    if session.last_seen.elapsed().as_secs() >= 20 && session.status != SessionStatus::Pending {
        return "нет связи".into();
    }
    match session.status {
        SessionStatus::Pending => "ждёт разрешения".into(),
        SessionStatus::Active => "доступен".into(),
        SessionStatus::Denied => "доступ отклонён".into(),
        SessionStatus::Captcha => "капча".into(),
        SessionStatus::Paused => "пауза".into(),
        SessionStatus::Offline => "нет связи".into(),
    }
}

fn action(html: &mut String, state: &AppState, session: &Session, op: &str, label: &str) {
    let _ = write!(
        html,
        "<form method=\"post\" action=\"/action\"><input type=\"hidden\" name=\"token\" value=\"{}\"><input type=\"hidden\" name=\"session\" value=\"{}\"><input type=\"hidden\" name=\"op\" value=\"{}\"><button>{}</button></form>",
        escape(&state.token),
        escape(&session.id),
        escape(op),
        escape(label)
    );
}

pub fn rename_page(state: &AppState, session: &Session) -> String {
    let mut html = String::new();
    head(&mut html, false, &state.token);
    let _ = write!(
        html,
        "<body><main><header><h1>Переименовать подключение</h1></header><section class=\"card\"><p class=\"meta\">{}</p>",
        escape(&session.full_name)
    );
    let _ = write!(
        html,
        "<form method=\"post\" action=\"/action\"><input type=\"hidden\" name=\"token\" value=\"{}\"><input type=\"hidden\" name=\"session\" value=\"{}\"><input type=\"hidden\" name=\"op\" value=\"rename\"><label>Название<input name=\"label\" value=\"{}\" autofocus></label><div class=\"actions\"><button>Сохранить</button><a class=\"button\" href=\"/?token={}\">Отмена</a></div></form></section></main></body></html>",
        escape(&state.token),
        escape(&session.id),
        escape(&session.label),
        escape(&state.token)
    );
    html
}

fn head(html: &mut String, refresh: bool, token: &str) {
    let _ = write!(
        html,
        "<!doctype html><html lang=\"ru\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>WordCraft Teacher</title>"
    );
    if refresh {
        let _ = write!(html, "<meta http-equiv=\"refresh\" content=\"2\">");
    }
    let _ = write!(html, "<style>{}</style></head>", CSS.replace("__TOKEN__", token));
}

fn ago(at: Instant, now: Instant) -> String {
    let seconds = now.saturating_duration_since(at).as_secs();
    if seconds < 2 {
        "только что".into()
    } else if seconds < 60 {
        format!("{seconds} с назад")
    } else if seconds < 3600 {
        format!("{} мин назад", seconds / 60)
    } else {
        format!("{} ч назад", seconds / 3600)
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

const CSS: &str = r#"
:root{color-scheme:light dark;--bg:#f3f5f7;--card:#fff;--text:#1d232b;--muted:#68717d;--line:#d8dee5;--accent:#4b63d8;--ok:#167a4a;--bad:#b3261e;--warn:#925a00}
@media (prefers-color-scheme:dark){:root{--bg:#15181d;--card:#1e232a;--text:#e8edf3;--muted:#9aa5b1;--line:#333b45;--accent:#8ca0ff;--ok:#64d39a;--bad:#ff8b84;--warn:#f0bc63}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:15px/1.45 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif}main{max-width:980px;margin:0 auto;padding:28px 20px 48px}h1{font-size:26px;margin:0}h2{font-size:18px;margin:26px 0 10px}.sub,.hint,.empty,.muted{color:var(--muted)}.sub{margin:4px 0 0}.hint{font-size:13px}.urls{display:flex;flex-wrap:wrap;gap:8px;margin:8px 0}code{padding:4px 7px;border-radius:6px;background:var(--card);border:1px solid var(--line);font:14px ui-monospace,SFMono-Regular,Menlo,monospace}section{margin-top:18px}.card{background:var(--card);border:1px solid var(--line);border-radius:12px;padding:14px 16px;margin:10px 0}.who{display:flex;align-items:center;gap:8px;flex-wrap:wrap}.status{font-size:12px;padding:2px 8px;border-radius:999px;border:1px solid var(--line)}.status.on{color:var(--ok);border-color:currentColor}.status.off{color:var(--muted)}.meta{font-size:12.5px;color:var(--muted);margin-top:4px;word-break:break-word}.suspect,.bad{margin-top:9px;padding:8px 10px;border-radius:8px;border:1px solid currentColor;color:var(--bad);background:color-mix(in srgb,var(--bad) 8%,transparent)}.captcha{margin-top:9px;color:var(--warn)}.notice{margin:16px 0;padding:11px 14px;border-radius:9px;border:1px solid var(--ok);color:var(--ok);background:color-mix(in srgb,var(--ok) 8%,transparent)}.actions{display:flex;flex-wrap:wrap;gap:7px;margin-top:12px;align-items:center}button,.button{appearance:none;font:inherit;font-size:13.5px;padding:7px 12px;border-radius:7px;border:1px solid var(--line);background:var(--bg);color:var(--text);text-decoration:none;cursor:pointer}button:hover,.button:hover{border-color:var(--accent)}form{display:inline}.events{display:grid;gap:4px}.event{display:flex;gap:12px;padding:6px 0;border-bottom:1px solid var(--line);font-size:13.5px}.event span{color:var(--muted);min-width:96px}label{display:grid;gap:6px;margin:14px 0;font-size:13.5px}input[name=label]{font:inherit;padding:9px 10px;border-radius:7px;border:1px solid var(--line);background:var(--bg);color:var(--text);max-width:420px}footer{margin-top:26px;font-size:13px}footer a{color:var(--accent)}a.button{display:inline-block}
"#;
