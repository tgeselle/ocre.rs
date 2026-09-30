use std::sync::LazyLock;

use crate::i18n::{Catalog, Locales};

const NL: &str = r#"nl:
  date:
    month_names: "jan,feb,mrt"
    abbr_day_names: "Zo,Ma,Di,Wo,Do,Vr,Za"
    formats:
      short: "%a %-d %b"
  number:
    format:
      delimiter: "'"
    currency:
      format:
        format: "%n %u"
"#;

static LOCALES: Locales =
    LazyLock::new(|| Catalog::load(&[("en", "en:\n"), ("fr", "fr:\n"), ("nl", NL), ("ru", "ru:\n")]));

#[test]
fn localizes_dates_and_times_with_named_formats() {
    let en = LOCALES.locale("en");
    assert_eq!(en.l("2026-09-29", "default"), "2026-09-29");
    assert_eq!(en.l("2026-09-29 14:05:00", "default"), "Tue, 29 Sep 2026 14:05:00 +0000", "a time uses time.formats");
    assert_eq!(en.l(1_790_690_700, "short"), "29 Sep 14:05", "Unix seconds are a time");
    assert_eq!(en.l("2026-09-29 14:05:00", "%I:%M %p"), "02:05 PM");
    assert_eq!(en.l("not a date", "long"), "not a date");
    assert_eq!(en.l("2026-09-29", "huge"), "translation missing: en.date.formats.huge");
    let fr = LOCALES.locale("fr");
    assert_eq!(fr.l("2026-09-29 14:05:00", "long"), "mardi 29 septembre 2026 14h05");
    assert_eq!(fr.l("2026-02-01", "%a %b"), "dim févr.");
    assert_eq!(LOCALES.locale("ru").l("2026-09-29", "long"), "September 29, 2026", "no built-in Russian: English");
}

#[test]
fn locale_files_override_names_and_formats() {
    let nl = LOCALES.locale("nl");
    assert_eq!(nl.l("2026-09-29", "short"), "Di 29 sep", "a list of 3 months is ignored");
    assert_eq!(nl.l("2026-09-29", "long"), "29 september 2026", "built-in format of the language");
}

#[test]
fn localizes_numbers() {
    let (en, fr, nl) = (LOCALES.locale("en"), LOCALES.locale("fr"), LOCALES.locale("nl"));
    assert_eq!(en.number("-1234567.891"), "-1,234,567.891");
    assert_eq!(fr.number(1234567), "1\u{a0}234\u{a0}567");
    assert_eq!(nl.number(1234.5), "1'234,5", "delimiter from the file, separator built in");
    assert_eq!(fr.number_with_precision(1234.567, 1), "1234,6");
    assert_eq!(fr.number_with_precision("n/a", 1), "n/a");
    assert_eq!(fr.currency(-0.001, "€"), "0,00\u{a0}€", "no sign when it rounds to zero");
    assert_eq!(fr.currency(-12, "€"), "-12,00\u{a0}€");
    assert_eq!(nl.currency(1234.5, "EUR"), "1'234,50 EUR");
    assert_eq!(en.currency("free", "$"), "free");
}

#[test]
fn distances_in_words() {
    let fr = LOCALES.locale("fr");
    let words = |from: &str, to: &str| fr.distance_of_time_in_words(from, to);
    assert_eq!(words("2026-01-01 10:00:00", "2026-01-01 10:00:10"), "moins d'une minute");
    assert_eq!(words("2026-01-01 10:00:00", "2026-01-01 10:01:00"), "1 minute");
    assert_eq!(words("2026-01-01 10:00:00", "2026-01-01 10:20:00"), "20 minutes");
    assert_eq!(words("2026-01-01", "2027-06-01"), "plus d'un an");
    assert_eq!(words("2026-01-01", "soon"), "2026-01-01");
    assert_eq!(fr.time_ago_in_words(crate::now() - 2 * 86_400), "2 jours");
    assert_eq!(LOCALES.locale("en").time_ago_in_words(crate::now()), "less than a minute");
}

#[test]
fn precision_keeps_text_that_is_not_a_number() {
    let fr = LOCALES.locale("fr");
    assert_eq!(fr.number_with_precision("1234.5", 1), "1234,5");
    assert_eq!(fr.number_with_precision("n/a", 1), "n/a");
}
