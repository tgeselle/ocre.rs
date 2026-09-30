use serde::Deserialize;

use super::*;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Live,
    Sandbox,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Id(u64);

#[derive(Debug, Deserialize, PartialEq)]
struct Settings {
    name: String,
    small: i8,
    medium: i16,
    count: i32,
    big: i64,
    byte: u8,
    port: u16,
    size: u32,
    id: Id,
    ratio: f32,
    rate: f64,
    initial: char,
    on: bool,
    off: bool,
    mode: Mode,
    tags: Vec<String>,
    ports: Vec<u16>,
    missing: Option<String>,
    present: Option<u32>,
    #[serde(default)]
    defaulted: u32,
    #[serde(rename = "api_url")]
    url: String,
}

const FULL: [(&str, &str); 20] = [
    ("NAME", "shop"),
    ("SMALL", "-8"),
    ("MEDIUM", "-16"),
    ("COUNT", " 32 "),
    ("BIG", "-64"),
    ("BYTE", "8"),
    ("PORT", "8787"),
    ("SIZE", "32"),
    ("ID", "7"),
    ("RATIO", "0.5"),
    ("RATE", "1.25"),
    ("INITIAL", "o"),
    ("ON", "TRUE"),
    ("OFF", "0"),
    ("MODE", "sandbox"),
    ("TAGS", "a, b,,c "),
    ("PORTS", "80,443"),
    ("PRESENT", "3"),
    ("API_URL", "https://api.example"),
    ("UNUSED", "x"),
];

#[test]
fn every_field_reads_its_upper_case_variable_as_its_type() {
    let settings: Settings = from_vars(FULL).unwrap();
    assert_eq!(
        settings,
        Settings {
            name: "shop".into(),
            small: -8,
            medium: -16,
            count: 32,
            big: -64,
            byte: 8,
            port: 8787,
            size: 32,
            id: Id(7),
            ratio: 0.5,
            rate: 1.25,
            initial: 'o',
            on: true,
            off: false,
            mode: Mode::Sandbox,
            tags: vec!["a".into(), "b".into(), "c".into()],
            ports: vec![80, 443],
            missing: None,
            present: Some(3),
            defaulted: 0,
            url: "https://api.example".into(),
        }
    );
    let live: Vec<(&str, &str)> = FULL.iter().map(|&(k, v)| if k == "MODE" { (k, "live") } else { (k, v) }).collect();
    assert_eq!(from_vars::<Settings>(live).unwrap().mode, Mode::Live);
    let yes: Vec<(&str, &str)> = FULL.iter().map(|&(k, v)| if k == "OFF" { (k, "false") } else { (k, v) }).collect();
    assert!(!from_vars::<Settings>(yes).unwrap().off);
}

fn error_of<T: DeserializeOwned + std::fmt::Debug>(vars: &[(&str, &str)]) -> String {
    match from_vars::<T>(vars.iter().copied()).unwrap_err() {
        Error::Internal(message) => message,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_missing_variable_names_both_places_to_set_it() {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Stripe {
        stripe_key: String,
    }
    let message = error_of::<Stripe>(&[]);
    assert!(message.starts_with("Worker variable or secret `STRIPE_KEY` is missing."), "{message}");
    assert!(message.contains("`STRIPE_KEY: bindings.text(\"...\"),`"), "{message}");
    assert!(message.contains("ocre secrets push STRIPE_KEY --file .prod.vars"), "{message}");
    assert!(message.ends_with("in `ocre dev`, add `STRIPE_KEY=...` to .dev.vars"), "{message}");
}

#[test]
fn a_value_of_the_wrong_type_names_the_variable_and_the_expected_type() {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Paging {
        per_page: u32,
    }
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Flag {
        enabled: bool,
    }
    let message = error_of::<Paging>(&[("PER_PAGE", "-1")]);
    assert!(message.starts_with("Worker variable `PER_PAGE` is `-1`, not a positive integer."), "{message}");
    let message = error_of::<Flag>(&[("ENABLED", "yes")]);
    assert!(message.starts_with("Worker variable `ENABLED` is `yes`, not `true` or `false`."), "{message}");
}

#[test]
fn only_structs_can_be_read() {
    assert!(error_of::<u32>(&[]).contains("reads a struct with named fields"));
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    enum Kind {
        A,
    }
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct WithEnum {
        kind: Kind,
    }
    assert!(error_of::<WithEnum>(&[("KIND", "B")]).contains("unknown variant `B`"));
}

#[test]
fn the_environment_follows_the_build_profile() {
    assert_eq!(Environment::current(), Environment::Development, "tests are debug builds");
    assert!(Environment::current().is_development());
    assert_eq!(Environment::Development.as_str(), "development");
    assert_eq!(Environment::Production.as_str(), "production");
    assert_eq!(ConfigError("x".into()).to_string(), "x");
}
