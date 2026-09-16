//! RustaSea ORM — models, fluent query builder, migrations, factories, vector support.
//!
//! Sprint 03 (M2) scope: fluent SQL-building core with typed errors, table naming
//! conventions, and feature-gated driver dialects (Postgres/MySQL/SQLite) plus the
//! `vector` feature for pgvector similarity search.

pub mod activity;
pub mod blueprint;
pub mod builder;
pub mod cache;
pub mod casts;
pub mod clause;
pub mod connections;
pub mod db;
pub mod eager;
pub mod error;
pub mod execution;
pub mod factory;
pub mod m2;
pub mod migration;
pub mod migration_guard;
pub mod model;
pub mod model_ops;
pub(crate) mod model_sql;
pub mod naming;
pub mod profile;
pub mod relation;
pub mod relations;
pub mod schema;
pub mod scopes;
pub mod sluggable;
pub mod tx;
pub mod types;
pub mod value;
pub mod vector;

pub use activity::{
    activity_recorder, changed_columns, clear_activity_recorder, clear_causer_resolver,
    current_batch_uuid, current_causer, filter_columns, object_keys, register_activity_recorder,
    set_batch_uuid, set_causer_resolver, ActivityColumnMode, ActivityColumns, ActivityEvent,
    ActivityLogError, ActivityOperation, ActivityRecorder, BatchScope,
};
pub use blueprint::Blueprint;
pub use builder::{Executor, Lock, OrderDirection, QueryBuilder, Raw};
pub use cache::{
    bump_table_generation, cache_store, clear_cache_store, flush_model_cache, query_cache_key,
    register_cache_store, table_generation, QueryCacheStore,
};
pub use casts::{
    BooleanCast, CastBinding, CastsAttributes, DateTimeCast, EncryptedCast, FloatCast, IntegerCast,
    JsonCast, NullableCast, StringCast,
};
pub use connections::{
    ConnectionConfig, ConnectionPair, ConnectionResolver, DatabaseConfig, DatabaseConnection,
    MigrationsConfig, PoolConfig, RedisConfig, RedisConnection, RedisOptions,
};
pub use db::{DbPool, PoolSettings};
pub use eager::EagerPlan;
pub use error::{ConnectionError, OrmError, Result, SchemaError, UpsertError};
pub use execution::{
    chunk_by, count_sql, raw, raw_sql, sum_sql, to_row_count_sql, transaction, Links, PageMeta,
    PaginationMeta, Paginator,
};
pub use factory::{Factory, FactoryState, SequenceFactory, SqlSeeder, User};
pub use m2::{InsertBuilder, ModelScopes, UpsertBuilder};
pub use migration::{
    register_migration, register_seeder, registered_migrator, Migration, MigrationError,
    MigrationRecord, Migrator, Seeder,
};
pub use migration_guard::{
    guard_destructive_command, is_production, DestructiveCommandRefused, PRODUCTION,
};
pub use model::{JsonSpec, Model, Relation, RelationKind, Relations, SoftDeletes, Timestamps};
pub use model_ops::{ModelOps, SluggableFind};
pub use naming::snake_plural;
pub use profile::{
    clear_query_recorder, query_recorder, register_query_recorder, QueryRecorder, SqlQueryEvent,
};
pub use schema::{Column, ColumnKind, DefaultValue, Schema, SchemaBlueprint};
pub use scopes::{
    register_global_scope, registered_global_scopes, reset_global_scopes,
    try_registered_global_scopes, CallbackScope, GlobalScope, GlobalScopeEntry,
    GlobalScopeRegistry, Scope, ScopeRegistry, SoftDeletesScope,
};
pub use sluggable::{slugify, SlugOptions};
pub use tx::{Transaction, TransactionError};
pub use types::{ColumnType, JsonFilter, JsonRelationFilter};
pub use value::Value;
pub use vector::{VectorMetric, VectorSimilarity};
