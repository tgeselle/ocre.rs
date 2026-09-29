//! `attachment` fields in `ocre g model|scaffold|api`, and the R2
//! bucket `ocre deploy` creates, run through the real binary.

#[path = "../support/mod.rs"]
mod support;

use std::fs;

use serde_json::json;
use support::Sandbox;

const BUCKET: &str = "[[r2_buckets]]\nbinding = \"STORAGE\"\nbucket_name = \"shop-storage\"\n";

#[test]
fn scaffold_with_attachments_writes_multipart_forms_and_file_routes() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) =
        sandbox.json(&["g", "scaffold", "Photo", "title:string", "image:attachment", "notes:attachment?"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["updated"], json!(["src/lib.rs", "wrangler.toml"]));
    let wrangler = fs::read_to_string(root.join("wrangler.toml")).unwrap();
    assert!(wrangler.ends_with(BUCKET), "{wrangler}");

    let migration = fs::read_to_string(root.join("migrations/0001_create_photos.sql")).unwrap();
    assert!(migration.contains(
        "    image_key TEXT NOT NULL,\n    image_filename TEXT NOT NULL,\n    image_content_type TEXT NOT NULL,\n    image_size INTEGER NOT NULL,\n    notes_key TEXT,\n"
    ), "{migration}");

    let model = fs::read_to_string(root.join("src/models/photo.rs")).unwrap();
    for expected in [
        "use ocre::{Ctx, Error, IntoParam, Page, Result, Validator, params, storage::{self, Attachment, Rules, Upload}};",
        "pub const IMAGE: Rules = Rules {\n    max_bytes: 10 * 1024 * 1024,\n    content_types: &[\"image/png\", \"image/jpeg\", \"image/gif\", \"image/webp\", \"application/pdf\", \"text/plain\"],\n};",
        "    pub image_size: i64,\n    pub notes_key: Option<String>,",
        "    #[serde(skip)]\n    pub image: Option<Upload>,",
        "    /// `Some(Some(file))` replaces the stored file, `Some(None)` removes it (the old one is deleted from R2).\n    #[serde(skip)]\n    pub notes: Option<Option<Upload>>,",
        "        v.check(\"image\", self.image.is_none(), \"can't be blank\");\n        if let Some(image) = &self.image {\n            v.file(\"image\", image, &IMAGE);\n        }",
        "        if let Some(Some(notes)) = &self.notes {\n            v.file(\"notes\", notes, &NOTES);\n        }",
        "    pub fn notes(&self) -> Option<Attachment> {\n        Some(Attachment {\n            key: self.notes_key.clone()?,",
        "        Some(upload) => Some(storage::store(ctx, \"photos/image\", upload).await?),",
        "    params.extend(storage::columns(image.as_ref()));",
        "INSERT INTO photos (title, image_key, image_filename, image_content_type, image_size, notes_key, notes_filename, notes_content_type, notes_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        "image_key = CASE WHEN ?3 THEN ?4 ELSE image_key END, image_filename = CASE WHEN ?3 THEN ?5 ELSE image_filename END",
        "notes_size = CASE WHEN ?8 THEN ?12 ELSE notes_size END, updated_at = datetime('now') WHERE id = ?13",
        "    params.extend(storage::column_changes(notes.as_ref().map(Option::as_ref)));",
        "[image.as_ref().map(|_| old.image()), notes.as_ref().and_then(|_| old.notes())] } else { [image, notes.flatten()] };",
        "DELETE FROM photos WHERE id = ?1 RETURNING *",
        "    storage::delete_attachments(ctx, &[Some(record.image()), record.notes()]).await?;",
    ] {
        assert!(model.contains(expected), "missing {expected}\n{model}");
    }

    let controller = fs::read_to_string(root.join("src/photos.rs")).unwrap();
    for expected in [
        "use axum::{\n    Router,\n    extract::{Path, State},\n    http::{HeaderMap, StatusCode},",
        "render, storage::{self, Disposition, Multipart, MultipartForm, Upload}};",
        "        .route(\"/photos/{id}/image\", get(image_file))\n        .route(\"/photos/{id}/notes\", get(notes_file))",
        "const FORM_LIMIT: usize = photo::IMAGE.max_bytes + photo::NOTES.max_bytes + 1024 * 1024;",
        "        Ok(Self { image: multipart.file(\"image\"), notes: multipart.file(\"notes\"), ..multipart.form()? })",
        "    pub remove_notes: bool,",
        "            notes: if self.remove_notes { Some(None) } else { self.notes.clone().map(Some) },",
        "            ..Self::default()",
        "Multipart(multipart): Multipart<FORM_LIMIT>) -> Result<Response> {\n    let form = PhotoForm::from_multipart(multipart)?;",
        "    storage::serve(&ctx, &record.notes().or_404()?, &headers, Disposition::Inline).await",
    ] {
        assert!(controller.contains(expected), "missing {expected}\n{controller}");
    }
    assert!(!controller.contains("Form("), "{controller}");

    let form = fs::read_to_string(root.join("templates/photos/_form.html")).unwrap();
    assert!(form.contains(r#"<label>Image <input type="file" name="image" accept="image/png,image/jpeg,image/gif,image/webp,application/pdf,text/plain"></label>"#), "{form}");
    let edit = fs::read_to_string(root.join("templates/photos/edit.html")).unwrap();
    assert!(edit.contains(r#"<form action="/photos/{{ id }}" method="post" enctype="multipart/form-data">"#), "{edit}");
    assert!(edit.contains(r#"  <label><input type="checkbox" name="remove_notes" value="true"> Remove notes</label>"#));
    let new = fs::read_to_string(root.join("templates/photos/new.html")).unwrap();
    assert!(new.contains(r#"enctype="multipart/form-data""#) && !new.contains("remove_notes"), "{new}");
    let show = fs::read_to_string(root.join("templates/photos/show.html")).unwrap();
    assert!(show.contains(r#"<dd>{% let file = photo.image() %}<a href="/photos/{{ photo.id }}/image">{{ file.filename }}</a> ({{ file.human_size() }})</dd>"#), "{show}");
    assert!(show.contains(r#"<dd>{% if let Some(file) = photo.notes() %}<a href="/photos/{{ photo.id }}/notes">"#));
    let index = fs::read_to_string(root.join("templates/photos/index.html")).unwrap();
    assert!(index.contains("<td>{{ photo.image_filename }}</td><td>{% if let Some(file) = photo.notes() %}{{ file.filename }}{% endif %}</td>"), "{index}");

    // A second model with files reuses the bucket.
    let (report, ok) = sandbox.json(&["g", "model", "Avatar", "picture:attachment"], &root);
    assert!(ok, "{report}");
    assert_eq!(fs::read_to_string(root.join("wrangler.toml")).unwrap().matches("[[r2_buckets]]").count(), 1);
    // Without attachments nothing changes: plain forms, no bucket.
    let plain = sandbox.new_app("plain", &[]);
    let (report, ok) = sandbox.json(&["g", "scaffold", "Tag", "name:string"], &plain);
    assert!(ok && !report["updated"].as_array().unwrap().contains(&json!("wrangler.toml")), "{report}");
    let controller = fs::read_to_string(plain.join("src/tags.rs")).unwrap();
    assert!(controller.contains("    Form, Router,") && !controller.contains("storage"), "{controller}");
}

#[test]
fn a_wrangler_toml_without_trailing_newline_gets_the_bucket_on_its_own_lines() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let wrangler = fs::read_to_string(root.join("wrangler.toml")).unwrap();
    fs::write(root.join("wrangler.toml"), wrangler.trim_end()).unwrap();
    let (report, ok) = sandbox.json(&["g", "model", "Doc", "file:attachment"], &root);
    assert!(ok, "{report}");
    let wrangler = fs::read_to_string(root.join("wrangler.toml")).unwrap();
    assert!(wrangler.contains("\n\n# Files (`ocre::storage`"), "{wrangler}");
}

#[test]
fn api_attachments_are_optional_and_uploaded_with_put() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &["--api"]);
    let (report, ok) = sandbox.json(&["g", "api", "Document", "name:string", "file:attachment"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "attachment `file` must be optional in a JSON API");
    assert!(report["hint"].as_str().unwrap().contains("`file:attachment?`"), "{report}");
    assert!(!root.join("src/models/document.rs").exists(), "nothing written");

    let (report, ok) = sandbox.json(&["g", "api", "Document", "name:string", "file:attachment?", "--graphql"], &root);
    assert!(ok, "{report}");
    let module = fs::read_to_string(root.join("src/documents_api.rs")).unwrap();
    for expected in [
        "//! PUT /api/documents/{id}/file        multipart body with the file as `file` (`curl -X PUT -F file=@file`); replaces it",
        "    http::{HeaderMap, StatusCode},\n    response::Response,",
        "use ocre::{ApiResult, Created, Ctx, Error, Json, OptionExt, Page, FieldError, storage::{self, Disposition, Multipart}};",
        "        .route(\"/api/documents/{id}/file\", get(file_file).put(attach_file).delete(remove_file))",
        "const FILE_LIMIT: usize = document::FILE.max_bytes + 64 * 1024;",
        "    Ok(storage::serve(&ctx, &record.file().or_404()?, &headers, Disposition::Inline).await?)",
        "    let changes = DocumentChanges { file: Some(Some(upload)), ..Default::default() };",
        "    let changes = DocumentChanges { file: Some(None), ..Default::default() };",
        "    pub file_key: Option<String>,\n    pub file_filename: Option<String>,",
        "            file_size: record.file_size,",
        "            file: None,",
        "use async_graphql::{Context, InputObject, Object, SimpleObject};",
    ] {
        assert!(module.contains(expected), "missing {expected}\n{module}");
    }
    assert!(fs::read_to_string(root.join("wrangler.toml")).unwrap().ends_with(BUCKET));
}

#[test]
fn deploy_creates_the_bucket_when_missing() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.set("has_secret");
    let (report, ok) = sandbox.json(&["g", "model", "Photo", "image:attachment"], &root);
    assert!(ok, "{report}");

    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["provisioned"], json!(["R2 bucket shop-storage"]));
    let calls = sandbox.calls();
    let info = calls.iter().position(|call| call == "r2 bucket info shop-storage --json").unwrap();
    assert_eq!(calls[info + 1], "r2 bucket create shop-storage");
    assert!(calls.iter().position(|call| call.starts_with("deploy")).unwrap() > info + 1, "created before the deploy");

    // It exists now: nothing to create.
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok && report.get("provisioned").is_none(), "{report}");

    // An account without R2 gets told how to enable it; nothing is deployed.
    fs::remove_file(sandbox.work.join("../state/bucket_shop-storage")).unwrap();
    sandbox.set("r2_not_enabled");
    let before = sandbox.calls().len();
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert!(
        report["error"].as_str().unwrap().starts_with("`wrangler r2 bucket info shop-storage` failed:"),
        "{report}"
    );
    assert!(report["hint"].as_str().unwrap().contains("Storage & databases > R2"), "{report}");
    assert!(!sandbox.calls()[before..].iter().any(|call| call.starts_with("deploy")));
}

#[test]
fn bucket_errors_stop_the_deploy() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.set("has_secret");
    sandbox.json(&["g", "model", "Photo", "image:attachment"], &root);
    // A second entry naming the same bucket is checked once.
    let wrangler = fs::read_to_string(root.join("wrangler.toml")).unwrap();
    fs::write(
        root.join("wrangler.toml"),
        format!("{wrangler}\n[[r2_buckets]]\nbinding = \"OTHER\"\nbucket_name = \"shop-storage\"\n"),
    )
    .unwrap();
    sandbox.set("r2_create_fails");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "`wrangler r2 bucket create shop-storage` failed (exit status: 1)");
    assert_eq!(sandbox.calls().iter().filter(|call| call.starts_with("r2 bucket info")).count(), 1);

    // Any other failure of `info` names the login.
    fs::remove_file(sandbox.work.join("../state/r2_create_fails")).unwrap();
    sandbox.script("npx", "#!/bin/sh\necho 'Not logged in' >&2\nexit 1\n");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert!(report["hint"].as_str().unwrap().contains("ocre login"), "{report}");
}
