//! `attachment` fields in `ocre g model|scaffold|api`, and the R2
//! bucket `ocre deploy` creates, run through the real binary.

#[path = "../support/mod.rs"]
mod support;

use std::{fs, path::Path};

use serde_json::json;
use support::Sandbox;

const BUCKET: &str = "STORAGE: bindings.r2({ name: \"shop-storage\" }),\n";

fn config(root: &Path) -> String {
    fs::read_to_string(root.join("cloudflare.config.ts")).unwrap()
}

#[test]
fn scaffold_with_attachments_writes_multipart_forms_and_file_routes() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) =
        sandbox.json(&["g", "scaffold", "Photo", "title:string", "image:attachment", "notes:attachment?"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["updated"], json!(["src/lib.rs", "cloudflare.config.ts"]));
    assert!(config(&root).contains(BUCKET), "{}", config(&root));

    let migration = fs::read_to_string(root.join("migrations/0001_create_photos.sql")).unwrap();
    assert!(migration.contains(
        "    image_key TEXT NOT NULL,\n    image_filename TEXT NOT NULL,\n    image_content_type TEXT NOT NULL,\n    image_size INTEGER NOT NULL,\n    notes_key TEXT,\n"
    ), "{migration}");

    let model = fs::read_to_string(root.join("src/models/photo.rs")).unwrap();
    for expected in [
        "use ocre::{Ctx, Error, IntoParam, Page, Query, Result, Validator, params, storage::{self, Attachment, Rules, Upload}};",
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
        "const FORM_LIMIT: usize = photo::IMAGE.max_bytes as usize + photo::NOTES.max_bytes as usize + 1024 * 1024;",
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
    assert!(
        edit.contains(r#"<form action="{{ paths::show(id) }}" method="post" enctype="multipart/form-data">"#),
        "{edit}"
    );
    assert!(edit.contains(r#"  <label><input type="checkbox" name="remove_notes" value="true"> Remove notes</label>"#));
    let new = fs::read_to_string(root.join("templates/photos/new.html")).unwrap();
    assert!(new.contains(r#"enctype="multipart/form-data""#) && !new.contains("remove_notes"), "{new}");
    let show = fs::read_to_string(root.join("templates/photos/show.html")).unwrap();
    assert!(show.contains(r#"<dd>{% let file = photo.image() %}<a href="{{ paths::image(photo.id) }}" hx-boost="false">{{ file.filename }}</a> ({{ file.human_size() }})</dd>"#), "{show}");
    assert!(show.contains(
        r#"<dd>{% if let Some(file) = photo.notes() %}<a href="{{ paths::notes(photo.id) }}" hx-boost="false">"#
    ));
    let index = fs::read_to_string(root.join("templates/photos/index.html")).unwrap();
    assert!(index.contains("<td>{{ photo.image_filename }}</td><td>{% if let Some(file) = photo.notes() %}{{ file.filename }}{% endif %}</td>"), "{index}");

    // A second model with files reuses the bucket.
    let (report, ok) = sandbox.json(&["g", "model", "Avatar", "picture:attachment"], &root);
    assert!(ok, "{report}");
    assert_eq!(config(&root).matches("bindings.r2(").count(), 1);
    // Without attachments nothing changes: plain forms, no bucket.
    let plain = sandbox.new_app("plain", &[]);
    let (report, ok) = sandbox.json(&["g", "scaffold", "Tag", "name:string"], &plain);
    assert!(ok && !report["updated"].as_array().unwrap().contains(&json!("cloudflare.config.ts")), "{report}");
    let controller = fs::read_to_string(plain.join("src/tags.rs")).unwrap();
    assert!(controller.contains("    Form, Router,") && !controller.contains("storage"), "{controller}");
}

#[test]
fn a_worker_name_ocre_cannot_read_stops_the_bucket_and_writes_nothing() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let unreadable = config(&root).replacen("name: \"shop\",", "name: `${prefix}shop`,", 1);
    fs::write(root.join("cloudflare.config.ts"), &unreadable).unwrap();
    let (report, ok) = sandbox.json(&["g", "model", "Doc", "file:attachment"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "cloudflare.config.ts has no `worker.name` Ocre can read");
    assert!(report["hint"].as_str().unwrap().contains("`name: \"<app-name>\",`"), "{report}");
    assert_eq!(config(&root), unreadable);
    assert!(!root.join("src/models/doc.rs").exists(), "nothing written");
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
        "use ocre::{ApiResult, Created, Ctx, Error, Json, OptionExt, Page, PageLinks, FieldError, storage::{self, Disposition, Multipart}};",
        "        .route(\"/api/documents/{id}/file\", get(file_file).put(attach_file).delete(remove_file))",
        "const FILE_LIMIT: usize = document::FILE.max_bytes as usize + 64 * 1024;",
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
    assert!(config(&root).contains(BUCKET));
}

#[test]
fn deploy_creates_the_bucket_when_missing() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.set("has_secret");
    sandbox.remote_database("shop");
    let (report, ok) = sandbox.json(&["g", "model", "Photo", "image:attachment"], &root);
    assert!(ok, "{report}");
    sandbox.clear_calls();

    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["provisioned"], json!(["R2 bucket shop-storage"]));
    let calls = sandbox.calls();
    let get = calls.iter().position(|call| call == "cf r2 buckets get shop-storage").unwrap();
    assert_eq!(calls[get + 1], "cf r2 buckets create --name shop-storage");
    assert!(
        calls.iter().position(|call| call.starts_with("cf deploy")).unwrap() > get + 1,
        "created before the deploy"
    );

    // It exists now: nothing to create.
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok && report.get("provisioned").is_none(), "{report}");

    // An account without R2 gets told how to enable it; nothing is deployed.
    fs::remove_file(sandbox.work.join("../state/bucket_shop-storage")).unwrap();
    sandbox.set("r2_not_enabled");
    sandbox.clear_calls();
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    let error = report["error"].as_str().unwrap();
    assert!(
        error.starts_with(
            "`cf r2 buckets get shop-storage` failed: ┌ APIError\n│ [10042] Please enable R2 through the Cloudflare \
             Dashboard.\n│ 403 Forbidden · HTTP "
        ),
        "{error}"
    );
    assert!(report["hint"].as_str().unwrap().contains("Storage & databases > R2"), "{report}");
    assert!(
        !sandbox.calls().iter().any(|call| call.starts_with("cf r2 buckets create") || call.starts_with("cf deploy"))
    );
}

#[test]
fn bucket_errors_stop_the_deploy() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.set("has_secret");
    sandbox.remote_database("shop");
    sandbox.json(&["g", "model", "Photo", "image:attachment"], &root);
    // A second binding naming the same bucket is checked once.
    let with_other =
        config(&root).replace("// ocre:env", "OTHER: bindings.r2({ name: \"shop-storage\" }),\n\t\t\t// ocre:env");
    fs::write(root.join("cloudflare.config.ts"), &with_other).unwrap();
    sandbox.set("r2_create_fails");
    sandbox.clear_calls();
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "`cf r2 buckets create --name shop-storage` failed: ┌ Error\n│ r2_create_fails\n└");
    assert_eq!(sandbox.calls().iter().filter(|call| call.starts_with("cf r2 buckets get")).count(), 1);
    assert!(!sandbox.calls().iter().any(|call| call.starts_with("cf deploy")), "never deployed without its bucket");

    // Any other failure of `get` names the login.
    fs::remove_file(sandbox.work.join("../state/r2_create_fails")).unwrap();
    sandbox.set("r2_get_fails");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert_eq!(
        report["error"],
        "`cf r2 buckets get shop-storage` failed: ┌ APIError\n│ [10000] Authentication error\n│ 401 Unauthorized\n└"
    );
    assert!(report["hint"].as_str().unwrap().contains("ocre login"), "{report}");

    // A bucket name that is not a literal: the canonical form, before any call.
    fs::remove_file(sandbox.work.join("../state/r2_get_fails")).unwrap();
    fs::write(
        root.join("cloudflare.config.ts"),
        with_other.replace("{ name: \"shop-storage\" }),\n\t\t\t// ocre:env", "{ name: bucket }),\n\t\t\t// ocre:env"),
    )
    .unwrap();
    sandbox.clear_calls();
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "cloudflare.config.ts defines `OTHER` in a form Ocre cannot read");
    assert_eq!(report["hint"], "write it as a literal: `OTHER: bindings.r2({ name: \"<bucket>\" }),`");
    assert!(!sandbox.calls().iter().any(|call| call.starts_with("cf r2") || call.starts_with("cf deploy")));
}
