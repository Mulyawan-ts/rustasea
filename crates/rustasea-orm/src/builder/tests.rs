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

/// Verifies the scalar JSON relation predicate binds each value and emits the
/// dialect-shaped `IN` form.
#[test]
fn where_json_in_binds_values() {
    let values = vec!["a".to_string(), "b".to_string()];
    let qb = QueryBuilder::table("posts")
        .where_json_in("payload", "author.id", &values)
        .unwrap();
    assert_eq!(
        qb.bindings(),
        &[Value::Text("a".into()), Value::Text("b".into())]
    );
    let raw = qb.to_raw_sql().unwrap();
    match dialect() {
        "postgres" => assert!(
            raw.contains("(payload ->> 'author.id') IN ('a', 'b')"),
            "{raw}"
        ),
        "mysql" => assert!(
            raw.contains("JSON_UNQUOTE(JSON_EXTRACT(payload, '$.author.id')) IN ('a', 'b')"),
            "{raw}"
        ),
        "sqlite" => assert!(
            raw.contains(
                "json_valid(payload) AND json_extract(payload, '$.author.id') IN ('a', 'b')"
            ),
            "{raw}"
        ),
        other => panic!("unexpected dialect {other}"),
    }
}

/// Verifies an empty scalar value list degrades to `1 = 0`.
#[test]
fn where_json_in_empty_is_never_true() {
    let qb = QueryBuilder::table("posts")
        .where_json_in("payload", "author.id", &[])
        .unwrap();
    assert_eq!(qb.bindings(), &[] as &[Value]);
    assert!(
        qb.to_sql().unwrap().contains("WHERE 1 = 0"),
        "{}",
        qb.to_sql().unwrap()
    );
}

/// Verifies the array-overlap JSON relation predicate per dialect.
#[test]
fn where_json_contains_any_emits_overlap() {
    let values = vec!["x".to_string(), "y".to_string()];
    let qb = QueryBuilder::table("posts")
        .where_json_contains_any("payload", "tag_ids", &values)
        .unwrap();
    match dialect() {
        "postgres" => {
            assert_eq!(
                qb.bindings(),
                &[Value::Text("x".into()), Value::Text("y".into())]
            );
            let raw = qb.to_raw_sql().unwrap();
            assert!(
                raw.contains("((payload)::jsonb -> 'tag_ids') ?| ARRAY['x', 'y']"),
                "{raw}"
            );
        }
        "mysql" => {
            // MySQL binds the whole candidate set as one JSON-array literal.
            assert_eq!(qb.bindings().len(), 1);
            assert!(matches!(qb.bindings()[0], Value::Json(_)));
            let raw = qb.to_raw_sql().unwrap();
            assert!(
                raw.contains("JSON_OVERLAPS(JSON_EXTRACT(payload, '$.tag_ids'), CAST('[\"x\",\"y\"]' AS JSON))"),
                "{raw}"
            );
        }
        "sqlite" => {
            assert_eq!(
                qb.bindings(),
                &[Value::Text("x".into()), Value::Text("y".into())]
            );
            let raw = qb.to_raw_sql().unwrap();
            assert!(
                raw.contains("json_valid(payload) AND EXISTS (SELECT 1 FROM json_each(payload, '$.tag_ids') WHERE json_each.value IN ('x', 'y'))"),
                "{raw}"
            );
        }
        other => panic!("unexpected dialect {other}"),
    }
}

/// Verifies an empty array value list degrades to `1 = 0`.
#[test]
fn where_json_contains_any_empty_is_never_true() {
    let qb = QueryBuilder::table("posts")
        .where_json_contains_any("payload", "tag_ids", &[])
        .unwrap();
    assert_eq!(qb.bindings(), &[] as &[Value]);
    assert!(
        qb.to_sql().unwrap().contains("WHERE 1 = 0"),
        "{}",
        qb.to_sql().unwrap()
    );
}

/// Verifies single quotes in a JSON path are escaped in the emitted literal.
#[test]
fn where_json_in_escapes_path_quotes() {
    let filter = crate::types::JsonRelationFilter {
        path: "a'b".into(),
        values: vec!["v".to_string()],
    };
    let (sql, count) = filter.to_sql("payload", "sqlite").unwrap();
    assert_eq!(count, 1);
    assert!(sql.contains("'$.a''b'"), "{sql}");
    let (array_sql, _) = filter.to_sql_array("payload", "sqlite").unwrap();
    assert!(array_sql.contains("'$.a''b'"), "{array_sql}");
}

/// Verifies an unknown dialect is a typed error for both JSON forms.
#[test]
fn json_relation_filter_unknown_dialect_errors() {
    let filter = crate::types::JsonRelationFilter {
        path: "p".into(),
        values: vec!["v".to_string()],
    };
    assert!(matches!(
        filter.to_sql("payload", "oracle"),
        Err(OrmError::UnsupportedDriver(_))
    ));
    assert!(matches!(
        filter.to_sql_array("payload", "oracle"),
        Err(OrmError::UnsupportedDriver(_))
    ));
}

/// Verifies every dialect's scalar and array SQL explicitly (independent of the
/// compiled feature), so the Postgres/MySQL shapes are asserted on a SQLite
/// build too.
#[test]
fn json_relation_filter_all_dialect_shapes() {
    let scalar = crate::types::JsonRelationFilter {
        path: "author.id".into(),
        values: vec!["a".to_string(), "b".to_string()],
    };
    let (sql, count) = scalar.to_sql("payload", "postgres").unwrap();
    assert_eq!(count, 2);
    assert_eq!(sql, "(payload ->> 'author.id') IN ({}, {})");

    let (sql, count) = scalar.to_sql("payload", "mysql").unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        sql,
        "JSON_UNQUOTE(JSON_EXTRACT(payload, '$.author.id')) IN ({}, {})"
    );

    let (sql, count) = scalar.to_sql("payload", "sqlite").unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        sql,
        "json_valid(payload) AND json_extract(payload, '$.author.id') IN ({}, {})"
    );

    let array = crate::types::JsonRelationFilter {
        path: "tag_ids".into(),
        values: vec!["x".to_string(), "y".to_string()],
    };
    let (sql, count) = array.to_sql_array("payload", "postgres").unwrap();
    assert_eq!(count, 2);
    assert_eq!(sql, "((payload)::jsonb -> 'tag_ids') ?| ARRAY[{}, {}]");

    // MySQL collapses the candidate set into one JSON-array bind.
    let (sql, count) = array.to_sql_array("payload", "mysql").unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        sql,
        "JSON_OVERLAPS(JSON_EXTRACT(payload, '$.tag_ids'), CAST({} AS JSON))"
    );

    let (sql, count) = array.to_sql_array("payload", "sqlite").unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        sql,
        "json_valid(payload) AND EXISTS (SELECT 1 FROM json_each(payload, '$.tag_ids') WHERE json_each.value IN ({}, {}))"
    );
}
