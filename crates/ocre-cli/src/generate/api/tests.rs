use super::*;

const APP: &str = "[dependencies]\nocre = { git = \"https://x\" }\nserde = \"1\"\n";

#[test]
fn graphql_feature_and_dependency_are_added_once() {
    let once = with_graphql(APP).unwrap();
    assert_eq!(
        once,
        format!(
            "[dependencies]\nocre = {{ git = \"https://x\", features = [\"graphql\"] }}\n{GRAPHQL_DEP}\nserde = \"1\"\n"
        )
    );
    assert_eq!(with_graphql(&once).unwrap(), once, "idempotent");
    let api_mode = "ocre = { path = \"/o\", default-features = false, features = [\"x\"] }\n";
    assert_eq!(
        with_graphql(api_mode).unwrap(),
        format!(
            "ocre = {{ path = \"/o\", default-features = false, features = [\"graphql\", \"x\"] }}\n{GRAPHQL_DEP}\n"
        )
    );
    assert!(with_graphql("[dependencies]\nserde = \"1\"\n").is_err());
}

#[test]
fn schema_gains_each_resource_under_the_markers() {
    let posts = ModelNames::parse("Post").unwrap();
    let tags = ModelNames::parse("Tag").unwrap();
    let schema = add_to_schema(&new_schema("posts_api", &posts), "tags_api", &tags).unwrap();
    assert!(
        schema.contains(
            "    // ocre:graphql-queries\n    crate::tags_api::TagQuery,\n    crate::posts_api::PostQuery,\n"
        )
    );
    assert!(schema.contains(
        "    // ocre:graphql-mutations\n    crate::tags_api::TagMutation,\n    crate::posts_api::PostMutation,\n"
    ));
    assert!(add_to_schema("pub struct Query();", "tags_api", &tags).is_err());
}
