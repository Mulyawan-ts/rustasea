//! Unit tests for the fluent query builder core.

use super::*;

/// Verifies toSql emits parameterized SQL for a basic filtered query.
#[test]
fn builds_parameterized_sql() {
    let qb = QueryBuilder::table("users")
        .where_eq("email", "a@b.c")
        .where_null("deleted_at")
        .order_by("created_at", OrderDirection::Desc)
        .limit(10);
    assert_eq!(
        qb.to_sql().unwrap(),
        "SELECT * FROM users WHERE email = $1 AND deleted_at IS NULL ORDER BY created_at DESC LIMIT 10"
    );
}

/// Verifies toRawSql inlines literals and toSql never does.
#[test]
fn raw_sql_inlines_literals() {
    let qb = QueryBuilder::table("users").where_eq("name", "O'Brien");
    assert!(qb.to_sql().unwrap().contains("$1"));
    assert!(qb.to_raw_sql().unwrap().contains("'O''Brien'"));
}

/// Verifies strict upsert rejects empty uniqueBy before any round-trip.
#[test]
fn upsert_rejects_empty_unique_by() {
    let err = QueryBuilder::assert_upsert(&[], 1).unwrap_err();
    assert!(matches!(
        err,
        OrmError::Upsert(crate::error::UpsertError::EmptyUniqueBy)
    ));
}

/// Verifies pagination window math.
#[test]
fn paginator_computes_last_page() {
    let p: crate::execution::Paginator<u8> =
        crate::execution::Paginator::new(vec![1, 2, 3], 2, 3, 10);
    assert_eq!(p.last_page, 4);
}

/// Verifies where_json binds the filter value and emits dialect-shaped SQL.
#[test]
fn where_json_binds_filter_value() {
    let qb = QueryBuilder::table("users")
        .where_json(
            "settings",
            JsonFilter::PathEquals("theme".into(), "dark".into()),
        )
        .unwrap();
    assert_eq!(qb.bindings(), &[Value::Text("dark".into())]);
    let raw = qb.to_raw_sql().unwrap();
    match dialect() {
        "postgres" => assert!(raw.contains("settings ->> '{theme}' = 'dark'"), "{raw}"),
        "mysql" => assert!(
            raw.contains("JSON_UNQUOTE(JSON_EXTRACT(settings, '$.theme')) = 'dark'"),
            "{raw}"
        ),
        "sqlite" => assert!(
            raw.contains("json_extract(settings, '$.theme') = 'dark'"),
            "{raw}"
        ),
        other => panic!("unexpected dialect {other}"),
    }

    // `Contains` is Postgres/MySQL-only; SQLite has no native operator.
    let contains = QueryBuilder::table("users").where_json(
        "settings",
        JsonFilter::Contains(serde_json::json!({"a": 1})),
    );
    match dialect() {
        "postgres" => {
            let contains = contains.unwrap();
            assert_eq!(contains.bindings().len(), 1);
            assert!(matches!(contains.bindings()[0], Value::Json(_)));
            assert!(contains.to_raw_sql().unwrap().contains("@> '{\"a\":1}'"));
        }
        "mysql" => {
            let contains = contains.unwrap();
            assert_eq!(contains.bindings().len(), 1);
            assert!(contains
                .to_raw_sql()
                .unwrap()
                .contains("JSON_CONTAINS(settings"));
        }
        "sqlite" => {
            assert!(matches!(contains, Err(OrmError::UnsupportedDriver(_))));
        }
        other => panic!("unexpected dialect {other}"),
    }

    // KeyExists needs no placeholder.
    let key = QueryBuilder::table("users")
        .where_json("settings", JsonFilter::KeyExists("theme".into()))
        .unwrap();
    assert_eq!(key.bindings(), &[] as &[Value]);
    let key_raw = key.to_raw_sql().unwrap();
    match dialect() {
        "postgres" => assert!(key_raw.contains("settings ? 'theme'"), "{key_raw}"),
        "mysql" => assert!(
            key_raw.contains("JSON_CONTAINS_PATH(settings, 'one', '$.theme')"),
            "{key_raw}"
        ),
        "sqlite" => assert!(
            key_raw.contains("json_type(settings, '$.theme') IS NOT NULL"),
            "{key_raw}"
        ),
        other => panic!("unexpected dialect {other}"),
    }
}
