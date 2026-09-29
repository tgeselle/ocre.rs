use super::*;

fn cron(schedule: &str) -> String {
    parse_cron(schedule).unwrap_or_else(|err| panic!("{schedule}: {}", err.message))
}

#[test]
fn cron_expressions_pass_through_normalized() {
    assert_eq!(cron(" 0  3 * * * "), "0 3 * * *");
    assert_eq!(cron("*/15 * * * *"), "*/15 * * * *");
    assert_eq!(cron("0 9 * * MON#2"), "0 9 * * MON#2");
}

#[test]
fn english_phrases_become_cron() {
    for (phrase, expected) in [
        ("every minute", "* * * * *"),
        ("every 1 minute", "* * * * *"),
        ("every 15 minutes", "*/15 * * * *"),
        ("Every 5 mins", "*/5 * * * *"),
        ("every hour", "0 * * * *"),
        ("hourly", "0 * * * *"),
        ("every 1 hour", "0 * * * *"),
        ("every 6 hours", "0 */6 * * *"),
        ("daily", "0 0 * * *"),
        ("every day at 3am", "0 3 * * *"),
        ("every day at 4:00 pm", "0 16 * * *"),
        ("each day at 12:30am", "30 0 * * *"),
        ("at 18:45", "45 18 * * *"),
        ("every day at noon", "0 12 * * *"),
        ("midnight", "0 0 * * *"),
        ("noon every day", "0 12 * * *"),
        ("midnight on tuesdays", "0 0 * * TUE"),
        ("every monday at 9am", "0 9 * * MON"),
        ("every monday and friday at 9:30", "30 9 * * MON,FRI"),
        ("on sat, sun at 10 a.m.", "0 10 * * SAT,SUN"),
        ("every weekday at 6pm", "0 18 * * MON-FRI"),
        ("every weekend", "0 0 * * SAT,SUN"),
        ("weekly", "0 0 * * SUN"),
        ("every week at 8am", "0 8 * * SUN"),
        ("monthly", "0 0 1 * *"),
        ("every month at 2 am", "0 2 1 * *"),
        ("every day at midnight", "0 0 * * *"),
        ("every day at 12pm", "0 12 * * *"),
        ("every day at 12am", "0 0 * * *"),
    ] {
        assert_eq!(cron(phrase), expected, "{phrase}");
    }
}

#[test]
fn unknown_phrases_explain_both_forms() {
    for phrase in [
        "",
        "every 15 seconds",
        "every 60 minutes",
        "every 0 hours",
        "every 24 hours",
        "every 5 minutes at 3pm",
        "every hour at 3pm",
        "every minute at 1am",
        "every 2 days",
        "every funday",
        "every day at 25:00",
        "every day at 13pm",
        "every day at 9:75",
        "every day at soon",
        "midnight at 3am",
        "every x minutes",
        "every x hours",
        "0 3 * *",
    ] {
        let err = parse_cron(phrase).unwrap_err();
        assert_eq!(err.message, format!("invalid schedule `{phrase}`"));
        assert!(err.hint.unwrap().contains("\"every 15 minutes\""));
    }
}
