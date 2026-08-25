use super::*;
use std::time::Duration;

#[test]
fn parses_structured_filters_and_phrases() {
    let query = SearchQuery::parse(r#"kind:file ext:md path:desktop -"draft copy" needle"#);

    assert_eq!(query.terms, vec!["needle"]);
    assert_eq!(query.excluded_terms, vec!["copy", "draft"]);
    assert_eq!(
        query.filters,
        vec![
            QueryFilter::Path("desktop".to_string(), false),
            QueryFilter::Extension("md".to_string(), false),
            QueryFilter::Kind(QueryKind::File, false),
        ]
    );
}

#[test]
fn parses_tag_filters() {
    let query = SearchQuery::parse("tag:Important label:Client -tag:Later");

    assert_eq!(
        query.filters,
        vec![
            QueryFilter::Tag("client".to_string(), false),
            QueryFilter::Tag("important".to_string(), false),
            QueryFilter::Tag("later".to_string(), true),
        ]
    );
}

#[test]
fn parses_scope_filters_and_prefixes() {
    let query = SearchQuery::parse("@desktop scope:downloads -scope:trash scope:/Users/me/Work");

    assert_eq!(
        query.filters,
        vec![
            QueryFilter::Scope(QueryScope::Desktop, false),
            QueryFilter::Scope(QueryScope::Downloads, false),
            QueryFilter::Scope(QueryScope::Trash, true),
            QueryFilter::Scope(QueryScope::Path("/users/me/work".to_string()), false),
        ]
    );
}

#[test]
fn matches_named_and_path_scopes() {
    let desktop = FileRecord {
        id: gfm_types::FileId::new(gfm_types::VolumeId(1), 1),
        parent: None,
        path: "/Users/me/Desktop/report.md".into(),
        name: "report.md".to_string(),
        kind: FileKind::File,
        len: 0,
        created: None,
        modified: None,
        changed: None,
        hidden: false,
        tags: Vec::new(),
    };

    assert!(QueryScope::Desktop.matches(&desktop));
    assert!(QueryScope::Home.matches(&desktop));
    assert!(QueryScope::Path("/users/me/desktop".to_string()).matches(&desktop));
    assert!(!QueryScope::Downloads.matches(&desktop));
}

#[test]
fn parses_boolean_expression_tree() {
    let query = SearchQuery::parse(r#"(report OR invoice) AND NOT draft kind:file"#);
    let expression = query.expression.expect("expression");

    assert!(matches!(expression, QueryExpr::And(_)));
    assert_eq!(query.terms, vec!["draft", "invoice", "report"]);
    assert_eq!(
        query.filters,
        vec![QueryFilter::Kind(QueryKind::File, false)]
    );
}

#[test]
fn parses_content_proximity_queries() {
    let query = SearchQuery::parse("near:4:alpha,beta");

    assert_eq!(
        query.proximities,
        vec![QueryProximity {
            distance: 4,
            terms: vec!["alpha".to_string(), "beta".to_string()],
        }]
    );
    assert!(matches!(
        query.expression,
        Some(QueryExpr::Proximity(QueryProximity { distance: 4, .. }))
    ));
}

#[test]
fn rejects_invalid_calendar_dates() {
    assert!(DateComparison::parse("2026-02-29").is_none());
    assert!(DateComparison::parse("2024-02-29").is_some());
    assert!(DateComparison::parse("1969-12-31").is_none());
}

#[test]
fn computes_stable_date_bounds() {
    let time = time_from_date("1970-01-02").unwrap();

    assert_eq!(
        time.duration_since(UNIX_EPOCH).unwrap(),
        Duration::from_secs(86_400)
    );
}
