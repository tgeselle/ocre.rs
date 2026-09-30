//! `ocre domains`: the Worker's custom domains, `worker.domains` of
//! cloudflare.config.ts. `ocre deploy` publishes the Worker on them:
//! Cloudflare creates the DNS record and the certificate, for hostnames in
//! a zone of the same account. Custom domains are free and their requests
//! count like `workers.dev` ones.

use crate::{
    CliResult, config,
    output::{CliError, Report},
    project::Project,
};

/// What to do with the list.
pub enum Action {
    List,
    Add(String),
    Remove(String),
}

pub fn run(project: &Project, action: Action) -> CliResult {
    let config = project.config()?;
    let mut domains = config.domains()?;
    let command = match &action {
        Action::List => {
            return Ok(Report { domains: Some(domains), ..Report::new("domains") });
        }
        Action::Add(host) => {
            check_host(host)?;
            if domains.contains(host) {
                return Err(CliError::new(format!("{host} is already a custom domain of the Worker"))
                    .hint("run `ocre deploy` to publish the Worker on it"));
            }
            domains.push(host.clone());
            "domains add"
        }
        Action::Remove(host) => {
            if !domains.contains(host) {
                return Err(CliError::new(format!("{host} is not a custom domain of the Worker"))
                    .hint("`ocre domains` lists them"));
            }
            domains.retain(|domain| domain != host);
            "domains remove"
        }
    };
    std::fs::write(project.root.join(config::FILE), config.with_domains(&domains)?)?;
    let next = if matches!(action, Action::Add(_)) {
        "ocre deploy (creates the DNS record and certificate; the zone must be on this Cloudflare account)"
    } else {
        "ocre deploy (detaches the domain from the Worker)"
    };
    Ok(Report {
        updated: vec![config::FILE.to_owned()],
        domains: Some(domains),
        next: vec![next.to_owned()],
        ..Report::new(command)
    })
}

/// A hostname Cloudflare accepts as a custom domain: two lowercase labels
/// or more, of letters, digits and inner dashes; no wildcard.
fn check_host(host: &str) -> Result<(), CliError> {
    let labels: Vec<&str> = host.split('.').collect();
    let valid = labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        });
    if valid {
        return Ok(());
    }
    Err(CliError::new(format!("`{host}` is not a hostname Ocre can add as a custom domain"))
        .hint("give a lowercase hostname without scheme, path or wildcard, e.g. `ocre domains add www.example.com`"))
}
