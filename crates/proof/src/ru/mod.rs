//! Russian proofing.
//!
//! - **Spelling** against the Hunspell `ru_RU` dictionary installed on the system
//!   ([`dictionary`], found by [`crate::hunspell`]: `ru_RU.aff`/`ru_RU.dic` from the OS, a package
//!   manager or LibreOffice; it isn't bundled). Without it Russian text gets no spelling marks, as
//!   for Polish and Portuguese. A capitalized word may start a sentence and headings may be in
//!   capitals; words written in another script (Latin names, e-mail addresses) are left alone, and
//!   the user dictionary applies as in English.
//! - **Suggestions** tuned for Russian ([`suggest`]): the dictionary's `MAP` slips first (missing
//!   `ё` — `елка` → `ёлка`, `мед` → `мёд`), then the `REP` confusions, a missing space
//!   (`незнаю` → `не знаю`), one-letter typos, and a dictionary scan for anything further off.
//! - **Grammar** ([`grammar`]): cheap, reliable rules with Russian messages.

pub mod grammar;
mod suggest;
#[cfg(test)]
pub(crate) mod test_dict;

use std::sync::{Arc, RwLock};

use crate::affix::AffixDict;

pub use suggest::suggest;

/// The Russian dictionary: `None` until first use, then the installed one (or an empty one).
static DICT: RwLock<Option<Arc<AffixDict>>> = RwLock::new(None);

/// File names of a Russian Hunspell dictionary.
const NAMES: &[&str] = &["ru_RU", "ru-RU", "ru"];

/// The Russian dictionary: the installed Hunspell `ru_RU` (read on first use), one given with
/// [`install_dictionary`], or an empty one when none is available (Russian spelling is off then;
/// see [`has_dictionary`]).
pub fn dictionary() -> Arc<AffixDict> {
    if let Some(d) = DICT.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return d.clone();
    }
    let loaded = Arc::new(load_installed());
    let mut slot = DICT.write().unwrap_or_else(|e| e.into_inner());
    // Another thread (or an install) may have filled it meanwhile: that one wins.
    slot.get_or_insert(loaded).clone()
}

fn load_installed() -> AffixDict {
    let Some((aff, dic)) = crate::hunspell::find(NAMES) else {
        log::info!("Russian spelling: no Hunspell ru_RU dictionary installed");
        return AffixDict::default();
    };
    crate::hunspell::load(&aff, &dic).unwrap_or_else(|e| {
        log::warn!("Russian spelling: {e}");
        AffixDict::default()
    })
}

/// Use this Russian dictionary from now on (Hunspell `.aff` and `.dic` text, decoded to UTF-8),
/// for example one the user picked. On error the current dictionary stays.
pub fn install_dictionary(aff: &str, dic: &str) -> Result<(), String> {
    let d = AffixDict::from_hunspell(aff, dic, &[])?;
    *DICT.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(d));
    Ok(())
}

/// Forget the loaded dictionary, so the next check looks for an installed one again (after
/// [`crate::hunspell::set_dictionary_dir`] or a dictionary was installed).
pub fn reload_dictionary() {
    *DICT.write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Is a Russian dictionary available (spelling is checked)?
pub fn has_dictionary() -> bool {
    !dictionary().is_empty()
}

/// Is `c` a letter of the Cyrillic script (the only script the Russian dictionary covers)?
fn is_cyrillic_letter(c: char) -> bool {
    c.is_alphabetic()
        && (('\u{0400}'..='\u{04FF}').contains(&c)
            || ('\u{0500}'..='\u{052F}').contains(&c)
            || ('\u{2DE0}'..='\u{2DFF}').contains(&c)
            || ('\u{A640}'..='\u{A69F}').contains(&c))
}

/// Is `word` spelled correctly in Russian? Numbers, single letters, short words in capitals
/// (abbreviations: `РФ`, `СССР`), words with digits and words in other scripts are accepted, and so
/// is every word when no Russian dictionary is installed.
pub fn is_correct(word: &str) -> bool {
    is_correct_with(&dictionary(), word)
}

/// [`is_correct`] against a given dictionary (an empty one accepts everything).
pub fn is_correct_with(d: &AffixDict, word: &str) -> bool {
    if d.is_empty() {
        return true; // no Russian dictionary installed: no spelling marks
    }
    let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’' && c != '-');
    let w = w.trim_matches(['\'', '’', '-']);
    let n = w.chars().count();
    if n <= 1 || w.chars().any(|c| c.is_ascii_digit()) || !w.chars().any(char::is_alphabetic) {
        return true;
    }
    if w.chars().all(|c| !c.is_lowercase()) && n <= 6 {
        return true;
    }
    if !w.chars().any(is_cyrillic_letter) {
        return true; // another script (a Latin word): not the Russian dictionary's business
    }
    let w = w.replace('’', "'");
    if w.contains('-') {
        return w.split('-').all(|p| p.chars().count() <= 1 || known(d, p));
    }
    known(d, &w)
}

/// Is one word (no hyphens) in the dictionary or the user dictionary? Words without Cyrillic
/// letters are accepted (they belong to another language).
fn known(d: &AffixDict, w: &str) -> bool {
    if !w.chars().any(is_cyrillic_letter) {
        return true;
    }
    d.check(w) || crate::in_user_dictionary(&w.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dictionary_loads() {
        test_dict::install();
        let d = dictionary();
        assert!(has_dictionary());
        assert!(d.len() > 50, "{}", d.len());
        assert!(d.rule_count() >= 5, "the test dictionary's affix rules");
        assert!(!d.rep().is_empty() && !d.map().is_empty() && !d.try_chars().is_empty());
    }

    #[test]
    fn without_a_dictionary_nothing_is_flagged() {
        let none = AffixDict::default();
        for w in ["малако", "сабака", "xqzv", ""] {
            assert!(is_correct_with(&none, w), "{w}");
        }
        assert!(install_dictionary("FLAG long\n", "1\nкот\n").is_err(), "unsupported files are refused");
    }

    #[test]
    fn russian_words_are_correct() {
        test_dict::install();
        for w in [
            "привет",
            "мир",
            "кот",
            "кошка",
            "собака",
            "молоко",
            "корова",
            "Москва",
            "Россия",
            "человек",
            "работа",
            "время",
            "день",
            "сегодня",
            "завтра",
            "язык",
            "слово",
            "книга",
            "стол",
            "рука",
            "вода",
            "хлеб",
            "дом",
            "друг",
            "дети",
            "жизнь",
            "ёлка",
            "ёж",
            "мёд",
            "чёрный",
            "жёлтый",
            "ещё",
            "не",
            "знаю",
            "я",
            "и",
            "в",
            "на",
            "США",
            "2026",
            "e-mail",
            "по-русски",
        ] {
            assert!(is_correct(w), "{w}");
        }
    }

    #[test]
    fn misspellings_are_flagged() {
        test_dict::install();
        for w in ["малако", "сабака", "карова", "харшо", "пажалуста", "чилавек", "здаровье", "нравица"]
        {
            assert!(!is_correct(w), "{w}");
        }
    }

    #[test]
    fn other_scripts_are_left_alone() {
        test_dict::install();
        assert!(is_correct("hello") && is_correct("WordCraft") && is_correct("naïve"), "Latin text isn't Russian");
    }

    #[test]
    fn user_dictionary_applies() {
        test_dict::install();
        assert!(!is_correct("zxqрусскоеслово"));
        crate::add_word("Zxqрусскоеслово");
        assert!(is_correct("zxqрусскоеслово"));
    }
}
