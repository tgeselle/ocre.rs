//! `ocre g model`: queries, callbacks, eager loading and every kind of
//! association, run through the real binary.

#[path = "../support/mod.rs"]
mod support;

use std::fs;

use serde_json::json;
use support::Sandbox;

#[test]
fn models_query_through_the_builder_and_call_their_callbacks() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["g", "model", "Post", "title:string"], &root);
    assert!(ok, "{report}");
    let model = fs::read_to_string(root.join("src/models/post.rs")).unwrap();
    for expected in [
        "use ocre::{Ctx, Error, Page, Query, Result, Validator, params};",
        "pub fn query() -> Query<Post> {\n    Query::table(\"posts\")\n}",
        "    query().order_desc(\"id\").page(page).all(&ctx.db()?).await",
        "    query().count(&ctx.db()?).await",
        "    query().eq(\"id\", id).first(&ctx.db()?).await",
        "        rows.extend(query().is_in(\"id\", chunk.iter().copied()).all(&db).await?);",
        "pub async fn create(ctx: &Ctx, mut new: NewPost) -> Result<Post> {\n    before_create(ctx, &mut new).await?;",
        "    after_create(ctx, &record).await?;\n    Ok(record)\n}",
        "    before_update(ctx, id, &mut changes).await?;",
        "    if let Some(record) = &updated {\n        after_update(ctx, record).await?;\n    }\n    Ok(updated)",
        "    before_delete(ctx, id).await?;\n    let deleted: Option<Post> = ctx.db()?.first(\"DELETE FROM posts WHERE id = ?1 RETURNING *\", params![id]).await?;",
        "async fn before_create(_ctx: &Ctx, _new: &mut NewPost) -> Result<()> {\n    Ok(())\n}",
        "async fn after_delete(_ctx: &Ctx, _post: &Post) -> Result<()> {",
    ] {
        assert!(model.contains(expected), "missing {expected}\n{model}");
    }
}

#[test]
fn references_give_has_one_has_many_through_and_preloads() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    for args in [["g", "model", "User", "email:string^"], ["g", "model", "Tag", "label:string^"]] {
        assert!(sandbox.json(&args, &root).1);
    }
    let (report, ok) = sandbox.json(&["g", "model", "Profile", "bio:text", "user:references^"], &root);
    assert!(ok, "{report}");
    let (report, ok) = sandbox.json(&["g", "model", "Post", "title:string", "user:references?"], &root);
    assert!(ok, "{report}");
    let (report, ok) = sandbox.json(&["g", "model", "Tagging", "post:references", "tag:references"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["updated"], json!(["src/models/post.rs", "src/models/tag.rs", "src/models/mod.rs"]));

    let user = fs::read_to_string(root.join("src/models/user.rs")).unwrap();
    assert!(user.contains(
        "    pub async fn profile(&self, ctx: &Ctx) -> Result<Option<crate::models::profile::Profile>> {\n        crate::models::profile::query().eq(\"user_id\", self.id).first(&ctx.db()?).await"
    ), "{user}");
    assert!(user.contains("pub async fn posts(&self, ctx: &Ctx, page: ocre::Page)"), "{user}");

    let post = fs::read_to_string(root.join("src/models/post.rs")).unwrap();
    for expected in [
        "    pub async fn taggings(&self, ctx: &Ctx, page: ocre::Page) -> Result<Vec<crate::models::tagging::Tagging>> {",
        "    /// Tags of this post, through taggings, most recently linked first.\n    pub async fn tags(&self, ctx: &Ctx, page: ocre::Page) -> Result<Vec<crate::models::tag::Tag>> {",
        "\"SELECT tags.* FROM tags JOIN taggings ON taggings.tag_id = tags.id WHERE taggings.post_id = ?1 ORDER BY taggings.id DESC LIMIT ?2 OFFSET ?3\"",
        "pub async fn preload_users(\n    ctx: &Ctx,\n    records: &[Post],\n) -> Result<std::collections::HashMap<i64, crate::models::user::User>> {\n    let mut ids: Vec<i64> = records.iter().filter_map(|record| record.user_id).collect();",
        "pub async fn for_users(ctx: &Ctx, user_ids: &[i64]) -> Result<Vec<Post>> {",
        "        rows.extend(query().is_in(\"user_id\", chunk.iter().copied()).order_desc(\"id\").all(&db).await?);",
    ] {
        assert!(post.contains(expected), "missing {expected}\n{post}");
    }
    let tag = fs::read_to_string(root.join("src/models/tag.rs")).unwrap();
    assert!(
        tag.contains(
            "pub async fn posts(&self, ctx: &Ctx, page: ocre::Page) -> Result<Vec<crate::models::post::Post>>"
        ),
        "{tag}"
    );

    let sql = fs::read_to_string(root.join("migrations/0005_create_taggings.sql")).unwrap();
    assert!(
        sql.ends_with("CREATE UNIQUE INDEX index_taggings_on_post_id_and_tag_id ON taggings (post_id, tag_id);\n"),
        "{sql}"
    );
    let sql = fs::read_to_string(root.join("migrations/0004_create_posts.sql")).unwrap();
    assert!(sql.contains("user_id INTEGER REFERENCES users(id) ON DELETE SET NULL"), "{sql}");
    let tagging = fs::read_to_string(root.join("src/models/tagging.rs")).unwrap();
    assert!(tagging.contains(
        "    v.check(\"tag_id\", query().eq(\"post_id\", new.post_id).eq(\"tag_id\", new.tag_id).exists(&db).await?, \"has already been taken\");"
    ), "{tagging}");
    assert!(tagging.contains("    let mut ids: Vec<i64> = records.iter().map(|record| record.post_id).collect();"));
}

#[test]
fn enum_fields_become_rust_enums_stored_as_text() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) =
        sandbox.json(&["g", "scaffold", "Task", "title:string", "state:enum:todo,in_progress", "mood:enum:ok?"], &root);
    assert!(ok, "{report}");
    let sql = fs::read_to_string(root.join("migrations/0001_create_tasks.sql")).unwrap();
    assert!(sql.contains("    state TEXT NOT NULL CHECK (state IN ('todo', 'in_progress')),\n"), "{sql}");
    let model = fs::read_to_string(root.join("src/models/task.rs")).unwrap();
    for expected in [
        "pub enum State {\n    #[default]\n    #[serde(rename = \"todo\")]\n    Todo,\n    #[serde(rename = \"in_progress\")]\n    InProgress,\n}",
        "    pub const ALL: [Self; 2] = [Self::Todo, Self::InProgress];",
        "            Self::InProgress => \"in_progress\",",
        "impl ocre::IntoParam for Mood {",
        "    pub state: State,\n    #[serde(default, deserialize_with = \"ocre::optional\")]\n    pub mood: Option<Mood>,",
    ] {
        assert!(model.contains(expected), "missing {expected}\n{model}");
    }
    let controller = fs::read_to_string(root.join("src/tasks.rs")).unwrap();
    assert!(controller.contains("v.one_of(\"state\", &self.state).unwrap_or_default()"), "{controller}");
    assert!(controller.contains("v.optional_one_of(\"mood\", &self.mood)"), "{controller}");

    let (report, _) = sandbox.json(&["g", "model", "Job", "result:enum:ok,failed"], &root);
    assert_eq!(report["error"], "enum `result` would be named `Result`, a name the model already uses");
    let (report, _) = sandbox.json(&["g", "api", "Ticket", "state:enum:open,closed", "--graphql"], &root);
    assert_eq!(report["error"], "enum `state` is not supported with --graphql yet");
}
