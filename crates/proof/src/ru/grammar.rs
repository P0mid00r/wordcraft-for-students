//! Russian grammar and punctuation checks: cheap, rule-based and tuned to stay quiet when unsure
//! (a missed mistake costs less than a wrong mark on correct text). Messages are in Russian.
//!
//! - **Repeated word** (`это это`), except interjections and words Russian doubles on purpose
//!   (`да да`, `ну ну`, `вот вот`, `еле еле`).
//! - **Space before punctuation** (`слово ,`) and **double spaces**; emoticons (` :)`) and
//!   ellipses are left alone.
//! - **Capital letter at the start of a sentence**, knowing Russian abbreviations (`т. е.`,
//!   `г.`, `ул.`, `др.`, `т. п.`), initials, ordinals (`5. место`), ellipses and dialogue dashes
//!   (`— Иди — сказал он`), after which lower case is correct.
//! - **Doubled punctuation** (`,,`, `..`, `;;`).
//!
//! No comma rule (Russian commas depend on clause structure) and no English rule (such as
//! `a`/`an`) applies to Russian text.

use crate::{Issue, IssueKind, ProofLang, words};

/// Words Russian repeats on purpose: interjections, onomatopoeia and emphatic doubling.
const ALLOWED_REPEATS: &[&str] = &[
    "да",
    "нет",
    "ну",
    "вот",
    "так",
    "очень",
    "еле",
    "чуть",
    "просто",
    "всё",
    "тихо",
    "ещё",
    "ай",
    "ой",
    "ах",
    "ох",
    "эй",
    "ух",
    "ну-ну",
    "да-да",
    "нет-нет",
    "ха",
    "хи",
    "хо",
];

/// Abbreviations written with a final period after which a sentence goes on in lower case (lower
/// case, without the period).
const ABBREVIATIONS: &[&str] = &[
    "т",
    "д",
    "г",
    "гг",
    "в",
    "вв",
    "с",
    "см",
    "стр",
    "рис",
    "табл",
    "др",
    "проф",
    "доц",
    "акад",
    "ул",
    "пер",
    "кв",
    "руб",
    "коп",
    "млн",
    "млрд",
    "тыс",
    "н",
    "э",
    "им",
    "гос",
    "напр",
    "ок",
    "тов",
    "просп",
    "пл",
    "обл",
    "р",
    "рр",
];

fn issue(start: usize, end: usize, message: String, suggestions: Vec<String>) -> Issue {
    Issue { start, end, kind: IssueKind::Grammar, message, suggestions, lang: ProofLang::Ru }
}

fn capitalize(w: &str) -> String {
    crate::affix::capitalize(w)
}

/// Is `prev`, before a period, an abbreviation, an initial, a number or a Roman numeral (after
/// which a lower-case word doesn't start a sentence)?
fn abbreviation_or_number(prev: &str) -> bool {
    prev.chars().count() <= 1
        || prev.chars().any(|c| c.is_ascii_digit())
        || (prev.chars().all(|c| matches!(c, 'I' | 'V' | 'X' | 'L' | 'C' | 'D' | 'M')))
        || ABBREVIATIONS.contains(&prev.to_lowercase().as_str())
}

const CLOSING: [char; 8] = ['"', '”', '’', '\'', ')', '»', ']', '\u{1}'];
const OPENING: [char; 8] = ['"', '“', '«', '(', '[', '\'', '‚', '—'];

/// Does the gap between two words end a sentence: a `.`, `!` or `?` (not an ellipsis, not after
/// an abbreviation), then space, with nothing but quotes and brackets around?
fn ends_sentence(gap: &str, prev: &str) -> bool {
    let t = gap.trim_start_matches(CLOSING);
    let Some(end) = t.chars().next() else { return false };
    if !matches!(end, '.' | '!' | '?') || t.starts_with("..") {
        return false;
    }
    if end == '.' && abbreviation_or_number(prev) {
        return false;
    }
    let rest = t.trim_start_matches(['.', '!', '?']).trim_start_matches(CLOSING);
    rest.starts_with(char::is_whitespace) && rest.trim_start().chars().all(|c| OPENING.contains(&c))
}

/// Grammar issues in a paragraph of Russian text, in order.
pub fn check(text: &str) -> Vec<Issue> {
    let ws: Vec<(usize, usize)> = words(text);
    let mut v = Vec::new();
    repeated_words(text, &ws, &mut v);
    spaces(text, &mut v);
    sentence_capitals(text, &ws, &mut v);
    doubled_punctuation(text, &mut v);
    v.sort_by_key(|i| (i.start, i.end));
    v
}

fn repeated_words(text: &str, ws: &[(usize, usize)], v: &mut Vec<Issue>) {
    for pair in ws.windows(2) {
        let [(a0, b0), (a1, b1)] = pair else { continue };
        let (Some(w0), Some(w1), Some(gap)) = (text.get(*a0..*b0), text.get(*a1..*b1), text.get(*b0..*a1)) else { continue };
        if !gap.is_empty()
            && gap.chars().all(char::is_whitespace)
            && w0.chars().all(char::is_alphabetic)
            && w0.to_lowercase() == w1.to_lowercase()
            && !ALLOWED_REPEATS.contains(&w0.to_lowercase().as_str())
        {
            v.push(issue(*a0, *b1, format!("Повтор слова: \"{w1}\""), vec![w0.to_string()]));
        }
    }
}

fn spaces(text: &str, v: &mut Vec<Issue>) {
    for (i, c) in text.char_indices() {
        if c != ' ' {
            continue;
        }
        let rest = text.get(i + 1..).unwrap_or("");
        let mut it = rest.chars();
        let Some(p) = it.next() else { continue };
        let after = it.next();
        let flag = match p {
            ',' => true,
            // `;)` and `;-)` are emoticons.
            ';' => !matches!(after, Some(')' | '(' | '-' | 'P' | 'D')),
            // ` ...` is an ellipsis, ` .5` a number.
            '.' => !matches!(after, Some('.')) && !after.is_some_and(|a| a.is_ascii_digit()),
            // ` :)`, ` :-)` and similar are emoticons; ` ?!` is still a stray space.
            ':' | '!' | '?' => after.is_none_or(|a| a.is_whitespace() || matches!(a, '"' | '”' | '’' | '!' | '?')),
            _ => false,
        };
        if flag {
            v.push(issue(i, i + 1 + p.len_utf8(), "Пробел перед знаком препинания".into(), vec![p.to_string()]));
        }
    }
    for (i, _) in text.match_indices("  ") {
        if i > 0 && text.get(..i).is_some_and(|t| t.ends_with(' ')) {
            continue;
        }
        v.push(issue(i, i + 2, "Двойной пробел".into(), vec![" ".into()]));
    }
}

fn sentence_capitals(text: &str, ws: &[(usize, usize)], v: &mut Vec<Issue>) {
    // List items (`– яблоки,`) may start in lower case.
    let list_item = text.trim_end().ends_with([',', ';']);
    for (k, &(a, b)) in ws.iter().enumerate() {
        let Some(w) = text.get(a..b) else { continue };
        let starts = if k == 0 {
            let before = text.get(..a).unwrap_or("");
            !list_item && before.chars().all(|c| c.is_whitespace() || OPENING.contains(&c))
        } else {
            let Some(&(pa, pb)) = ws.get(k - 1) else { continue };
            ends_sentence(text.get(pb..a).unwrap_or(""), text.get(pa..pb).unwrap_or(""))
        };
        if starts && w.chars().next().is_some_and(char::is_lowercase) && w.chars().all(char::is_alphabetic) {
            v.push(issue(a, b, "Слово в начале предложения с заглавной буквы".into(), vec![capitalize(w)]));
        }
    }
}

fn doubled_punctuation(text: &str, v: &mut Vec<Issue>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if !matches!(b, b',' | b'.' | b';') {
            i += 1;
            continue;
        }
        let run = bytes.get(i..).map_or(0, |r| r.iter().take_while(|&&x| x == b).count());
        let flag = match b {
            b'.' => run == 2,
            _ => run >= 2,
        };
        if flag {
            let end = i + run;
            let suggestions: Vec<String> = match b {
                b'.' => vec![".".into(), "…".into()],
                _ => vec![char::from(b).to_string()],
            };
            v.push(issue(i, end, "Повторяющийся знак препинания".into(), suggestions));
        }
        i += run.max(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(t: &str) -> Vec<String> {
        check(t).into_iter().map(|i| format!("{}@{}", i.message, &t[i.start..i.end])).collect()
    }

    fn clean(t: &str) {
        let m = messages(t);
        assert!(m.is_empty(), "{t:?}: {m:?}");
    }

    #[test]
    fn repeated_words() {
        let v = check("Это это было видно, и он он ушёл.");
        assert_eq!(v.len(), 2, "{v:?}");
        assert_eq!(v[0].message, "Повтор слова: \"это\"");
        assert_eq!(v[0].suggestions, ["Это"]);
        assert!(v.iter().all(|i| i.lang == ProofLang::Ru && i.kind == IssueKind::Grammar));
        clean("Ну ну, да да, вот вот, еле еле.");
    }

    #[test]
    fn spaces_and_punctuation() {
        let m = messages("Привет , мир .  Как дела ?");
        assert!(m.contains(&"Пробел перед знаком препинания@ ,".to_string()), "{m:?}");
        assert!(m.contains(&"Пробел перед знаком препинания@ .".to_string()), "{m:?}");
        assert!(m.contains(&"Пробел перед знаком препинания@ ?".to_string()), "{m:?}");
        assert!(m.contains(&"Двойной пробел@  ".to_string()), "{m:?}");
        clean("Класс :) Отлично :-) Значение .5 и многоточие ...");
        let m = messages("Сказал,, и ушёл.. потом;; конец.");
        assert_eq!(m.iter().filter(|x| x.starts_with("Повторяющийся знак")).count(), 3, "{m:?}");
        clean("Ждал… и ждал... пока он вернётся.");
    }

    #[test]
    fn sentence_start_capitals() {
        let m = messages("это тест. вторая фраза! третья? ладно.");
        assert_eq!(m.iter().filter(|x| x.starts_with("Слово в начале")).count(), 4, "{m:?}");
        assert_eq!(check("он прав.")[0].suggestions, ["Он"]);
        // Abbreviations, initials, ordinals, ellipses and dialogue dashes.
        clean("Купил фрукты, т. е. яблоки и груши.");
        clean("Встретил проф. Иванова на ул. Пушкина.");
        clean("Он занял 5. место в 2026 г.");
        clean("Ждал... и он появился.");
        clean("— Иди — сказал он. — Завтра я вернусь.");
        clean("Он сказал: \"Вернусь завтра\". И ушёл.");
        // A list item may start in lower case.
        clean("яблоки,");
    }

    #[test]
    fn correct_russian_text_is_clean() {
        clean("Съешь же ещё этих мягких французских булок, да выпей чаю.");
        clean("Москва, 12 октября 2026 года — памятная дата.");
    }

    proptest::proptest! {
        #[test]
        fn never_panics(s in "\\PC{0,80}") {
            let _ = check(&s);
        }

        #[test]
        fn never_panics_on_russian_like_text(s in "[а-яА-ЯёЁA-Za-z ,.;:!?\"…\\-—]{0,60}") {
            for i in check(&s) {
                proptest::prop_assert!(i.start <= i.end && i.end <= s.len());
                proptest::prop_assert!(s.is_char_boundary(i.start) && s.is_char_boundary(i.end));
            }
        }
    }
}
