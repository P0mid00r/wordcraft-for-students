//! A tiny synthetic Russian Hunspell dictionary for tests (written for these tests; not taken
//! from any real dictionary). Real dictionaries are installed on the system, never bundled.

/// Affix rules: a few suffixes and one prefix, plus the suggestion tables the Russian suggestions
/// use (`TRY`, `MAP` for `ё`, `REP` for common confusions).
pub const AFF: &str = "SET UTF-8
TRY аеиоунтсрвлкмдпзяыьгбчйхжшюцщэфъё
MAP 1
MAP её
REP 4
REP е ё
REP ё е
REP а о
REP о а
SFX A Y 1
SFX A 0 ы .
SFX B Y 2
SFX B 0 а .
SFX B 0 у .
SFX C Y 1
SFX C 0 ть .
SFX D Y 1
SFX D 0 ый .
SFX E Y 1
SFX E 0 о .
PFX N Y 1
PFX N 0 не .
";

/// Stems (the count line first, as in Hunspell files).
pub const DIC: &str = "65
привет
мир
кот
кошка
собака
молоко
корова
Москва
Россия
человек
работа
время
день
сегодня
завтра
язык
слово
книга
стол
рука
вода
хлеб
дом
друг
дети
жизнь
ёлка
ёж
мёд
чёрный
жёлтый
ещё
не
знаю
я
и
в
на
США
по
русски
хорошо
пожалуйста
здоровье
конституция
окно
дверь
город
страна
школа
студент
учитель
урок
письмо
вопрос
ответ
семья
мать
отец
брат
сестра
голова
нога
глаз
сердце
";

/// Make [`AFF`]/[`DIC`] the Russian dictionary (every test that needs one calls this; they all
/// install the same one, so tests running in parallel agree).
pub fn install() {
    if let Err(e) = super::install_dictionary(AFF, DIC) {
        panic!("test dictionary: {e}");
    }
}
