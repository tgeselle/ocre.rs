//! `public_id:token`: generated controllers put the public id in URLs. The
//! generators write their usual code, keyed by the integer `id`; these
//! rewrites make the `{id}` path segment the public id, resolved by an `Id`
//! extractor (one query) into the integer id the model functions take.

use crate::names::ModelNames;

/// Rewrites of the scaffold controller (HTML pages): links and redirects
/// carry the public id too.
pub(super) fn html(controller: &str, names: &ModelNames, many: &[String]) -> String {
    let ModelNames { singular, .. } = names;
    let code = controller
        .replace("Redirect::to(&paths::show(record.id))", "Redirect::to(&paths::show(&record.public_id))")
        .replace("Redirect::to(&paths::show(id))", "Redirect::to(&paths::show(&key))")
        .replace(&format!("paths::show({singular}.id) }}}}"), &format!("paths::show({singular}.public_id) }}}}"))
        .replace("paths::show(id)`.", "paths::show(public_id)`.")
        .replace(
            "struct EditView {\n    id: i64,",
            "struct EditView {\n    /// The public id, for the form's action.\n    id: String,",
        )
        .replace("render(&EditView { id, ", "render(&EditView { id: key, ")
        .replace(
            &format!("realtime::remove(&format!(\"{singular}_{{id}}\"))"),
            &format!("realtime::remove(&format!(\"{singular}_{{key}}\"))"),
        );
    let code = handlers(&code, many);
    insert_extractor(&code, names, "ocre::Error", "\n/// What the new and edit forms submit")
}

/// Rewrites of the JSON API: errors are JSON, and records serialize without
/// their integer id.
pub(super) fn api(module: &str, names: &ModelNames, many: &[String]) -> String {
    let code = handlers(module, many);
    insert_extractor(&code, names, "ocre::ApiError", "\n/// One page (`?limit=&offset=`)")
}

/// Rewrites of `ocre g resource` (index and show, HTML or JSON).
pub(super) fn resource(code: &str, names: &ModelNames, json: bool) -> String {
    let rejection = if json { "ocre::ApiError" } else { "ocre::Error" };
    insert_extractor(&handlers(code, &[]), names, rejection, "\nasync fn index(")
}

/// Handlers take `Id(id, key)` instead of the integer path segment, and the
/// files of `photos:attachments` are found by their own public id.
fn handlers(code: &str, many: &[String]) -> String {
    let mut code = code.replace("Path(id): Path<i64>", "Id(id, key): Id").replace(
        "Path((id, file_id)): Path<(i64, i64)>",
        "Id(id, key): Id,\n    Path((_, file_id)): Path<(String, String)>",
    );
    for name in many {
        let one = crate::names::singularize(name).expect("checked when parsed");
        code = code
            .replace(
                &format!("async fn find_{one}(ctx: &Ctx, id: i64, file_id: i64)"),
                &format!("async fn find_{one}(ctx: &Ctx, id: i64, file_id: &str)"),
            )
            .replace(&format!("find_{one}(&ctx, id, file_id)"), &format!("find_{one}(&ctx, id, &file_id)"));
    }
    code = code.replace("query().eq(\"id\", file_id)", "query().eq(\"public_id\", file_id)").replace(
        "/// The row of file `file_id` of record `id`.",
        "/// The row of file `file_id` (its public id) of record `id`.",
    );
    // Handlers that do not link to the record take the id alone.
    code.split_inclusive("\n}\n")
        .map(|item| {
            let uses_key = item.matches("key").count() > item.matches("Id(id, key): Id").count();
            if uses_key { item.to_owned() } else { item.replace("Id(id, key): Id", "Id(id, ..): Id") }
        })
        .collect()
}

/// Adds the `Id` extractor before `anchor`; it rejects with `rejection`.
fn insert_extractor(code: &str, names: &ModelNames, rejection: &str, anchor: &str) -> String {
    let ModelNames { singular, .. } = names;
    let lower = names.human_singular.to_lowercase();
    // JSON handlers never link back: the extractor gives the integer id alone.
    if !code.contains("Id(id, key): Id") {
        let extractor = format!(
            r#"
/// The {lower} of the `{{id}}` path segment, its public id: `Id(id)` is the
/// integer id the model functions take. A 404 when no {lower} has it.
struct Id(i64);

impl axum::extract::FromRequestParts<Ctx> for Id {{
    type Rejection = {rejection};

    async fn from_request_parts(parts: &mut axum::http::request::Parts, ctx: &Ctx) -> Result<Self, Self::Rejection> {{
        let Path(params) = Path::<Vec<(String, String)>>::from_request_parts(parts, ctx).await.map_err(|_| ocre::Error::NotFound)?;
        let key = params.into_iter().find(|(name, _)| name == "id").map(|(_, value)| value).or_404()?;
        Ok(Self({singular}::find_by_public_id(ctx, &key).await?.or_404()?.id))
    }}
}}
"#
        );
        return code.replace("Id(id, ..): Id", "Id(id): Id").replacen(anchor, &format!("{extractor}{anchor}"), 1);
    }
    let extractor = format!(
        r#"
/// The {lower} of the `{{id}}` path segment, its public id: `Id(id, key)` is
/// the integer id the model functions take and the public id for links.
/// A 404 when no {lower} has it.
struct Id(i64, String);

impl axum::extract::FromRequestParts<Ctx> for Id {{
    type Rejection = {rejection};

    async fn from_request_parts(parts: &mut axum::http::request::Parts, ctx: &Ctx) -> Result<Self, Self::Rejection> {{
        let Path(params) = Path::<Vec<(String, String)>>::from_request_parts(parts, ctx).await.map_err(|_| ocre::Error::NotFound)?;
        let key = params.into_iter().find(|(name, _)| name == "id").map(|(_, value)| value).or_404()?;
        let record = {singular}::find_by_public_id(ctx, &key).await?.or_404()?;
        Ok(Self(record.id, key))
    }}
}}
"#
    );
    code.replacen(anchor, &format!("{extractor}{anchor}"), 1)
}
