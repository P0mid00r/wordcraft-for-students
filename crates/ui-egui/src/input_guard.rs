//! Pauses text input when typing is too fast to be a person.
//!
//! A program that types into WordCraft through the operating system looks like a keyboard at
//! every layer above the OS: `egui` turns both into `Event::Text`. Three signals are cheap and
//! work on every platform (including the web): a burst of characters in one frame, a sustained
//! rate no person reaches, and a metronomic rhythm. When one fires, text input is paused for a
//! minute; the document is left as it was.
//!
//! This is a deterrent, not proof. A program that types slowly or adds jitter passes, and a
//! person typing unusually fast or evenly can be paused too. The pause only stops input; the
//! document is left as it was, and commands that do not type (saving, closing) still run.

use std::collections::VecDeque;

use crate::WordApp;

const LOCKOUT_SECONDS: u64 = 60;
const LOCKOUT_MS: f64 = 60_000.0;
const WINDOW_MS: f64 = 1_000.0;
const MIN_SPAN_SECONDS: f64 = 0.5;

/// Characters per second over the window; the world record is under 20.
const MAX_CPS: f64 = 25.0;
/// Characters in one frame; a person cannot press a key that many times in ~16 ms.
const BURST_CHARS: usize = 12;
/// A run of frames with almost the same gap between them (a program posting on a timer).
const REGULAR_MIN_INTERVALS: usize = 8;
const REGULAR_MAX_MEAN_MS: f64 = 120.0;
const REGULAR_MAX_STDDEV_MS: f64 = 6.0;

/// The error a command returns while typing is paused.
pub const PAUSED: &str = "text input is paused";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PauseReason {
    #[default]
    Detected,
    Teacher,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suspicion {
    pub reason: String,
    pub detail: String,
}

/// Tracks text input and holds the one-minute pause.
pub struct InputGuard {
    /// Off in `cfg(test)`: existing tests inject whole strings in one frame. A test that wants
    /// the guard sets it to true.
    pub enabled: bool,
    /// Report a suspicion instead of pausing locally; the classroom teacher decides.
    pub report_only: bool,
    samples: VecDeque<(f64, usize)>,
    locked_until: Option<f64>,
    reason: PauseReason,
    expired: bool,
    skip_frame: bool,
    suspicion: Option<Suspicion>,
}

impl Default for InputGuard {
    fn default() -> Self {
        Self {
            enabled: !cfg!(test),
            report_only: false,
            samples: VecDeque::new(),
            locked_until: None,
            reason: PauseReason::Detected,
            expired: false,
            skip_frame: false,
            suspicion: None,
        }
    }
}

impl InputGuard {
    /// The control channel's events are programmatic: they must not teach the detector.
    pub fn note_programmatic_text(&mut self) {
        self.skip_frame = true;
    }

    /// Feed one frame. Returns true when typing is paused (now, or since before).
    pub fn observe(&mut self, now: f64, chars: usize, ime: bool, repeat: bool) -> bool {
        if std::mem::take(&mut self.skip_frame) {
            return false;
        }
        if self.locked(now) {
            return true;
        }
        if now <= 0.0 || chars == 0 || ime || repeat {
            return false;
        }
        self.samples.push_back((now, chars));
        while self.samples.front().is_some_and(|(at, _)| now - *at > WINDOW_MS) {
            self.samples.pop_front();
        }
        let total = self.samples.iter().fold(0usize, |sum, (_, n)| sum.saturating_add(*n));
        let oldest = self.samples.front().map(|(at, _)| *at).unwrap_or(now);
        let seconds = ((now - oldest) / 1000.0).max(MIN_SPAN_SECONDS);
        let cps = total as f64 / seconds;
        let reason = if chars >= BURST_CHARS {
            "burst"
        } else if cps > MAX_CPS {
            "fast"
        } else if self.metronomic() {
            "regular"
        } else {
            return false;
        };
        let detail = format!("{total} / {seconds:.1} s");
        self.samples.clear();
        if self.report_only {
            log::warn!("typing suspicion reported: {reason}, {detail}");
            self.suspicion = Some(Suspicion { reason: reason.into(), detail });
            return false;
        }
        self.locked_until = Some(now + LOCKOUT_MS);
        self.reason = PauseReason::Detected;
        log::warn!("text input paused: {detail}");
        true
    }

    pub fn take_suspicion(&mut self) -> Option<Suspicion> {
        self.suspicion.take()
    }

    pub fn pause_for(&mut self, now: f64, seconds: u64) {
        let seconds = seconds.clamp(1, LOCKOUT_SECONDS);
        self.samples.clear();
        self.locked_until = Some(now + seconds as f64 * 1000.0);
        self.reason = PauseReason::Teacher;
        self.expired = false;
    }

    pub fn release(&mut self) {
        self.locked_until = None;
        self.expired = false;
        self.samples.clear();
    }

    fn metronomic(&self) -> bool {
        if self.samples.len() < REGULAR_MIN_INTERVALS + 1 {
            return false;
        }
        let start = self.samples.len() - (REGULAR_MIN_INTERVALS + 1);
        let mut intervals = Vec::with_capacity(REGULAR_MIN_INTERVALS);
        let mut previous: Option<f64> = None;
        for (at, _) in self.samples.iter().skip(start) {
            if let Some(previous) = previous {
                intervals.push(at - previous);
            }
            previous = Some(*at);
        }
        let count = intervals.len();
        if count == 0 {
            return false;
        }
        let mean = intervals.iter().sum::<f64>() / count as f64;
        if mean <= 0.0 || mean > REGULAR_MAX_MEAN_MS {
            return false;
        }
        let variance = intervals.iter().map(|gap| (gap - mean) * (gap - mean)).sum::<f64>() / count as f64;
        variance.sqrt() < REGULAR_MAX_STDDEV_MS
    }

    pub fn locked(&self, now: f64) -> bool {
        self.locked_until.is_some_and(|until| now < until)
    }

    /// Expire the pause after its minute. Returns whether input is still paused.
    pub fn tick(&mut self, now: f64) -> bool {
        let Some(until) = self.locked_until else { return false };
        if now < until {
            return true;
        }
        self.locked_until = None;
        self.expired = true;
        self.samples.clear();
        false
    }

    /// Whether a pause ended since the last call; the canvas takes the keyboard back then.
    pub fn take_expired(&mut self) -> bool {
        std::mem::take(&mut self.expired)
    }

    pub fn remaining_seconds(&self, now: f64) -> u64 {
        let Some(until) = self.locked_until else { return 0 };
        let seconds = ((until - now).max(0.0) / 1000.0).ceil();
        if seconds.is_finite() { (seconds as u64).min(LOCKOUT_SECONDS) } else { LOCKOUT_SECONDS }
    }
}

/// Commands that would put text into the document, or otherwise change it, while paused.
pub fn blocks_command(id: &str) -> bool {
    id.starts_with("text.")
        || id.starts_with("equation.")
        || id.starts_with("edit.paste")
        || matches!(id, "edit.cut" | "edit.undo" | "edit.redo" | "ui.text" | "ui.key")
}

/// Draws the countdown; it closes itself when the minute is over.
pub fn show(app: &mut WordApp, ctx: &egui::Context) {
    let now = crate::now_ms();
    if !app.input_guard.locked(now) {
        if app.input_guard.take_expired() && app.dialog.is_none() {
            app.canvas.want_focus = true;
        }
        return;
    }
    let seconds = app.input_guard.remaining_seconds(now);
    let body = match app.input_guard.reason {
        PauseReason::Detected => crate::tl!("Unusually fast typing was detected. Text input is paused for {seconds} seconds."),
        PauseReason::Teacher => crate::tl!("Your teacher paused text input for {seconds} seconds."),
    };
    let _ = egui::Modal::new(egui::Id::new("wordcraft.inputPaused")).show(ctx, |ui| {
        ui.set_max_width(380.0);
        ui.heading(crate::tl!("Input paused"));
        ui.add_space(6.0);
        ui.label(crate::i18n::fmt(body, &[("seconds", &seconds.to_string())]));
        ui.add_space(10.0);
        let left = (seconds as f32 / LOCKOUT_SECONDS as f32).clamp(0.0, 1.0);
        ui.add(egui::ProgressBar::new(left).desired_width(340.0));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> InputGuard {
        InputGuard { enabled: true, ..Default::default() }
    }

    #[test]
    fn human_typing_is_not_paused() {
        let mut guard = guard();
        let jitter = [0.0, 17.0, -11.0, 9.0, -14.0, 21.0, -8.0, 12.0, -19.0, 6.0];
        let mut now = 1_000.0;
        for (i, offset) in jitter.iter().cycle().take(40).enumerate() {
            guard.observe(now, 1, false, false);
            now += 100.0 + offset;
            assert!(!guard.locked(now), "paused after {i} ordinary keystrokes");
        }
    }

    #[test]
    fn a_burst_pauses_input_for_a_minute() {
        let mut guard = guard();
        let start = 1_000.0;
        assert!(guard.observe(start, 100, false, false));
        assert!(guard.locked(start));
        assert_eq!(guard.remaining_seconds(start), 60);
        assert_eq!(guard.remaining_seconds(start + 30_000.0), 30);
        assert!(guard.tick(start + 59_000.0));
        assert!(!guard.tick(start + 60_000.0));
        assert!(guard.take_expired());
        assert!(!guard.locked(start + 60_000.0));
    }

    #[test]
    fn a_metronomic_rhythm_pauses_input() {
        let mut guard = guard();
        let mut now = 1_000.0;
        for _ in 0..10 {
            guard.observe(now, 1, false, false);
            now += 50.0;
        }
        assert!(guard.locked(now), "a program posting every 50 ms is not a person");
    }

    #[test]
    fn ime_commits_key_repeat_and_control_input_are_ignored() {
        let mut guard = guard();
        guard.observe(1_000.0, 100, true, false);
        assert!(!guard.locked(1_000.0));
        guard.observe(2_000.0, 100, false, true);
        assert!(!guard.locked(2_000.0));
        guard.note_programmatic_text();
        guard.observe(3_000.0, 100, false, false);
        assert!(!guard.locked(3_000.0));
        guard.observe(3_000.0, 100, false, false);
        assert!(guard.locked(3_000.0), "the flag covers one frame only");
    }

    #[test]
    fn a_twelve_character_frame_is_a_burst() {
        let mut guard = guard();
        assert!(guard.observe(1_000.0, BURST_CHARS, false, false));
    }

    #[test]
    fn zero_time_never_locks() {
        let mut guard = guard();
        assert!(!guard.observe(0.0, 100, false, false));
    }

    #[test]
    fn report_only_records_a_suspicion_without_locking() {
        let mut guard = guard();
        guard.report_only = true;
        assert!(!guard.observe(1_000.0, 100, false, false));
        assert!(!guard.locked(1_000.0));
        let suspicion = guard.take_suspicion().unwrap();
        assert_eq!(suspicion.reason, "burst");
        assert!(suspicion.detail.contains("100"));
    }

    #[test]
    fn a_teacher_pause_locks_and_can_be_released() {
        let mut guard = guard();
        guard.pause_for(1_000.0, 60);
        assert!(guard.locked(1_000.0));
        assert_eq!(guard.remaining_seconds(1_000.0), 60);
        guard.release();
        assert!(!guard.locked(1_000.0));
    }

    #[test]
    fn text_commands_are_blocked_but_saving_is_not() {
        assert!(blocks_command("text.insert"));
        assert!(blocks_command("text.backspace"));
        assert!(blocks_command("equation.type"));
        assert!(blocks_command("edit.paste"));
        assert!(blocks_command("edit.pasteSpecial"));
        assert!(blocks_command("edit.cut"));
        assert!(blocks_command("ui.text"));
        assert!(!blocks_command("file.save"));
        assert!(!blocks_command("edit.copy"));
        assert!(!blocks_command("insert.picture"));
    }
}
