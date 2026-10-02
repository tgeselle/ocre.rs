//! `ocre new --starter`: content added to a new app. Starters run the real
//! generators, so the app has the same code `ocre g` writes; the qa starter
//! then replaces a few generated files with its own (`templates/starters/qa/`).

use std::path::Path;

use crate::{
    generate::{self, AuthOptions},
    new::Starter,
    output::CliError,
    project::Project,
};

const EVENT_FIELDS: [&str; 3] = ["name:string", "public_id:token", "user:references"];
const QUESTION_FIELDS: [&str; 4] = ["event:references", "body:text", "votes:integer", "answered:boolean"];

/// The qa starter's own files, written over what the generators made.
const QA_FILES: &[(&str, &str)] = &[
    ("src/events.rs", include_str!("../templates/starters/qa/src/events.rs")),
    ("src/questions.rs", include_str!("../templates/starters/qa/src/questions.rs")),
    ("src/realtime.rs", include_str!("../templates/starters/qa/src/realtime.rs")),
    ("src/models/question.rs", include_str!("../templates/starters/qa/src/models/question.rs")),
    ("templates/layout.html", include_str!("../templates/starters/qa/templates/layout.html")),
    ("templates/home.html", include_str!("../templates/starters/qa/templates/home.html")),
    ("templates/events/index.html", include_str!("../templates/starters/qa/templates/events/index.html")),
    ("templates/events/show.html", include_str!("../templates/starters/qa/templates/events/show.html")),
    ("templates/events/_form.html", include_str!("../templates/starters/qa/templates/events/_form.html")),
    ("templates/questions/_list.html", include_str!("../templates/starters/qa/templates/questions/_list.html")),
    ("tests/app.rs", include_str!("../templates/starters/qa/tests/app.rs")),
    ("tests/events.rs", include_str!("../templates/starters/qa/tests/events.rs")),
    ("tests/questions.rs", include_str!("../templates/starters/qa/tests/questions.rs")),
];

/// Generated question pages the room replaces: questions live on the event page.
const QA_REMOVED: [&str; 6] = [
    "templates/questions/index.html",
    "templates/questions/show.html",
    "templates/questions/new.html",
    "templates/questions/edit.html",
    "templates/questions/_form.html",
    "templates/questions/_row.html",
];

/// Adds the starter to the new app at `root`; returns the files it created,
/// relative to `root`.
pub fn apply(starter: Starter, root: &Path, api: bool) -> Result<Vec<String>, CliError> {
    match starter {
        Starter::Empty => Ok(Vec::new()),
        Starter::Blog => blog(root, api),
        Starter::Qa => qa(root),
    }
}

/// A `Post` scaffold (or JSON API).
fn blog(root: &Path, api: bool) -> Result<Vec<String>, CliError> {
    let fields = strings(&["title:string", "body:text", "published:boolean"]);
    let generator = if api { "api" } else { "scaffold" };
    let project = project(root, &[&["g", generator, "Post"], &fields.iter().map(String::as_str).collect::<Vec<_>>()])?;
    let report = if api {
        generate::api(&project, "Post", &fields, false)?
    } else {
        generate::scaffold(&project, "Post", &fields, false)?
    };
    Ok(report.created)
}

/// The live Q&A app: `ocre g auth`, an `Event` scaffold owned by its host,
/// a live `Question` scaffold, then the room (see the tutorial).
fn qa(root: &Path) -> Result<Vec<String>, CliError> {
    let mut created = generate::auth(&project(root, &[&["g", "auth"]])?, &AuthOptions::default())?.created;
    let fields = strings(&EVENT_FIELDS);
    let project_event = project(root, &[&["g", "scaffold", "Event"], &EVENT_FIELDS])?;
    created.extend(generate::scaffold(&project_event, "Event", &fields, false)?.created);
    let fields = strings(&QUESTION_FIELDS);
    let project_question = project(root, &[&["g", "scaffold", "Question"], &QUESTION_FIELDS, &["--realtime"]])?;
    created.extend(generate::scaffold(&project_question, "Question", &fields, true)?.created);

    for (path, contents) in QA_FILES {
        std::fs::write(root.join(path), contents)?;
        if !created.iter().any(|created| created == path) {
            created.push((*path).to_owned());
        }
    }
    for path in QA_REMOVED {
        std::fs::remove_file(root.join(path))?;
        created.retain(|created| created != path);
    }
    Ok(created)
}

/// The app at `root`, as `ocre <invocation>` would see it (the invocation is
/// kept in the generation record).
fn project(root: &Path, invocation: &[&[&str]]) -> Result<Project, CliError> {
    let mut project = Project::at(root.to_path_buf())?;
    project.generate.invocation = invocation.iter().flat_map(|part| part.iter().map(|arg| (*arg).to_owned())).collect();
    Ok(project)
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}
