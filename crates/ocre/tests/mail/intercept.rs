use super::*;

fn email() -> Email {
    Email::new("ada@example.com", "News", "Hello")
}

#[test]
fn interceptors_run_in_order_and_can_drop_the_email() {
    let redirect: Arc<Interceptor> = Arc::new(|mut email: Email, vars: &dyn Fn(&str) -> Option<String>| {
        email.to = vec![vars("STAGING_INBOX")?];
        Some(email)
    });
    let marketing_off: Arc<Interceptor> =
        Arc::new(|email: Email, _: &dyn Fn(&str) -> Option<String>| (email.subject != "News").then_some(email));
    let vars = |name: &str| (name == "STAGING_INBOX").then(|| "qa@example.com".to_owned());

    let sent = run(std::slice::from_ref(&redirect), email(), &vars).unwrap();
    assert_eq!(sent.to, ["qa@example.com"]);
    assert!(run(&[redirect.clone(), marketing_off], email(), &vars).is_none());
    assert!(run(&[redirect], email(), &|_| None).is_none(), "no inbox: dropped");
    assert_eq!(run(&[], email(), &vars), Some(email()));
}

#[test]
fn registered_interceptors_apply_to_every_email() {
    intercept(|mut email, _| {
        email.headers.push(("X-Intercepted".into(), "1".into()));
        Some(email)
    });
    let sent = intercepted(email(), &|_| None).unwrap();
    assert!(sent.headers.contains(&("X-Intercepted".into(), "1".into())));
    assert_eq!(dropped_line(&sent), "[ocre mail] dropped by an interceptor: \"News\" to ada@example.com");
}
