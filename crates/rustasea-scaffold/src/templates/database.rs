//! Database layer: migrations, factories, and seeders.
//!
//! Mirrors Laravel's `database/{migrations,factories,seeders}`. Migrations are
//! reversible (`up` + `down`) per the project database standards.

use super::TemplateFile;

/// Database templates (shared by every variant).
pub fn entries() -> Vec<TemplateFile> {
    vec![
        ("database/mod.rs", DATABASE_MOD),
        ("database/migrations/mod.rs", MIGRATIONS_MOD),
        ("database/migrations/create_users.rs", CREATE_USERS),
        ("database/migrations/create_sessions.rs", CREATE_SESSIONS),
        (
            "database/migrations/create_password_reset_tokens.rs",
            CREATE_PASSWORD_RESET_TOKENS,
        ),
        ("database/migrations/create_roles.rs", CREATE_ROLES),
        (
            "database/migrations/create_permissions.rs",
            CREATE_PERMISSIONS,
        ),
        ("database/migrations/create_role_user.rs", CREATE_ROLE_USER),
        (
            "database/migrations/create_permission_role.rs",
            CREATE_PERMISSION_ROLE,
        ),
        ("database/migrations/create_audit_log.rs", CREATE_AUDIT_LOG),
        (
            "database/migrations/create_authentication_log.rs",
            CREATE_AUTHENTICATION_LOG,
        ),
        ("database/factories/mod.rs", FACTORIES_MOD),
        ("database/factories/user_factory.rs", USER_FACTORY),
        ("database/seeders/mod.rs", SEEDERS_MOD),
        ("database/seeders/database_seeder.rs", DATABASE_SEEDER),
    ]
}

const DATABASE_MOD: &str = r##"//! Database layer — migrations, factories, and seeders.

pub mod factories;
pub mod migrations;
pub mod seeders;
"##;

const MIGRATIONS_MOD: &str = r##"//! Versioned, reversible schema migrations.

pub mod create_audit_log;
pub mod create_authentication_log;
pub mod create_password_reset_tokens;
pub mod create_permission_role;
pub mod create_permissions;
pub mod create_role_user;
pub mod create_roles;
pub mod create_sessions;
pub mod create_users;
"##;

const CREATE_USERS: &str = r##"//! Creates the `users` table.

use rustasea::orm::Migration;
use rustasea::OrmResult;

/// `create_users_table` migration.
pub struct CreateUsers;

impl Migration for CreateUsers {
    fn name(&self) -> &str {
        "2027_01_01_000001_create_users_table"
    }

    fn up(&self) -> OrmResult<String> {
        Ok(r#"CREATE TABLE users (
    id UUID PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    email VARCHAR(255) NOT NULL UNIQUE,
    password VARCHAR(255) NOT NULL,
    email_verified_at TIMESTAMPTZ NULL,
    -- Two-factor secret, stored encrypted at rest (ciphertext in this column).
    two_factor_secret TEXT NULL,
    -- JSON array of single-use 2FA recovery codes, stored as text.
    two_factor_recovery_codes TEXT NULL,
    two_factor_confirmed_at TIMESTAMPTZ NULL,
    -- Remember-me token for persistent logins.
    remember_token VARCHAR(100) NULL,
    -- IANA timezone name for the user's wall-clock preferences.
    timezone VARCHAR(64) NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    deleted_at TIMESTAMPTZ NULL
);"#
        .to_string())
    }

    fn down(&self) -> OrmResult<String> {
        Ok("DROP TABLE IF EXISTS users;".to_string())
    }
}
"##;

const CREATE_SESSIONS: &str = r##"//! Creates the `sessions` table used by the browser session guard.

use rustasea::orm::Migration;
use rustasea::OrmResult;

/// `create_sessions_table` migration.
pub struct CreateSessions;

impl Migration for CreateSessions {
    fn name(&self) -> &str {
        "2027_01_01_000002_create_sessions_table"
    }

    fn up(&self) -> OrmResult<String> {
        Ok(r#"CREATE TABLE sessions (
    id VARCHAR(255) PRIMARY KEY,
    user_id UUID NULL,
    payload TEXT NOT NULL,
    last_activity BIGINT NOT NULL
);"#
        .to_string())
    }

    fn down(&self) -> OrmResult<String> {
        Ok("DROP TABLE IF EXISTS sessions;".to_string())
    }
}
"##;

const CREATE_PASSWORD_RESET_TOKENS: &str = r##"//! Creates the `password_reset_tokens` table.

use rustasea::orm::Migration;
use rustasea::OrmResult;

/// `create_password_reset_tokens_table` migration.
pub struct CreatePasswordResetTokens;

impl Migration for CreatePasswordResetTokens {
    fn name(&self) -> &str {
        "2027_01_01_000003_create_password_reset_tokens_table"
    }

    fn up(&self) -> OrmResult<String> {
        Ok(r#"CREATE TABLE password_reset_tokens (
    email VARCHAR(255) PRIMARY KEY,
    token VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL
);"#
        .to_string())
    }

    fn down(&self) -> OrmResult<String> {
        Ok("DROP TABLE IF EXISTS password_reset_tokens;".to_string())
    }
}
"##;

const CREATE_ROLES: &str = r##"//! Creates the `roles` table (RBAC).

use rustasea::orm::Migration;
use rustasea::OrmResult;

/// `create_roles_table` migration.
pub struct CreateRoles;

impl Migration for CreateRoles {
    fn name(&self) -> &str {
        "2027_01_01_000004_create_roles_table"
    }

    fn up(&self) -> OrmResult<String> {
        Ok(r#"CREATE TABLE roles (
    id UUID PRIMARY KEY,
    name VARCHAR(255) NOT NULL UNIQUE,
    guard VARCHAR(255) NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);"#
        .to_string())
    }

    fn down(&self) -> OrmResult<String> {
        Ok("DROP TABLE IF EXISTS roles;".to_string())
    }
}
"##;

const CREATE_PERMISSIONS: &str = r##"//! Creates the `permissions` table (RBAC).

use rustasea::orm::Migration;
use rustasea::OrmResult;

/// `create_permissions_table` migration.
pub struct CreatePermissions;

impl Migration for CreatePermissions {
    fn name(&self) -> &str {
        "2027_01_01_000005_create_permissions_table"
    }

    fn up(&self) -> OrmResult<String> {
        Ok(r#"CREATE TABLE permissions (
    id UUID PRIMARY KEY,
    name VARCHAR(255) NOT NULL UNIQUE,
    guard VARCHAR(255) NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);"#
        .to_string())
    }

    fn down(&self) -> OrmResult<String> {
        Ok("DROP TABLE IF EXISTS permissions;".to_string())
    }
}
"##;

const CREATE_ROLE_USER: &str = r##"//! Creates the `role_user` pivot table (RBAC user ↔ role assignments).

use rustasea::orm::Migration;
use rustasea::OrmResult;

/// `create_role_user_table` migration.
pub struct CreateRoleUser;

impl Migration for CreateRoleUser {
    fn name(&self) -> &str {
        "2027_01_01_000006_create_role_user_table"
    }

    fn up(&self) -> OrmResult<String> {
        Ok(r#"CREATE TABLE role_user (
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role_id UUID NOT NULL REFERENCES roles (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id, role_id)
);
CREATE INDEX role_user_role_id_index ON role_user (role_id);"#
        .to_string())
    }

    fn down(&self) -> OrmResult<String> {
        Ok("DROP TABLE IF EXISTS role_user;".to_string())
    }
}
"##;

const CREATE_PERMISSION_ROLE: &str = r##"//! Creates the `permission_role` pivot table (RBAC permission ↔ role grants).

use rustasea::orm::Migration;
use rustasea::OrmResult;

/// `create_permission_role_table` migration.
pub struct CreatePermissionRole;

impl Migration for CreatePermissionRole {
    fn name(&self) -> &str {
        "2027_01_01_000007_create_permission_role_table"
    }

    fn up(&self) -> OrmResult<String> {
        Ok(r#"CREATE TABLE permission_role (
    permission_id UUID NOT NULL REFERENCES permissions (id) ON DELETE CASCADE,
    role_id UUID NOT NULL REFERENCES roles (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (permission_id, role_id)
);
CREATE INDEX permission_role_role_id_index ON permission_role (role_id);"#
        .to_string())
    }

    fn down(&self) -> OrmResult<String> {
        Ok("DROP TABLE IF EXISTS permission_role;".to_string())
    }
}
"##;

const CREATE_AUDIT_LOG: &str = r##"//! Creates the `audit_log` table (activity/audit trail).

use rustasea::orm::Migration;
use rustasea::OrmResult;

/// `create_audit_log_table` migration.
pub struct CreateAuditLog;

impl Migration for CreateAuditLog {
    fn name(&self) -> &str {
        "2027_01_01_000008_create_audit_log_table"
    }

    fn up(&self) -> OrmResult<String> {
        Ok(r#"CREATE TABLE audit_log (
    id UUID PRIMARY KEY,
    batch_uuid UUID NULL,
    log_name VARCHAR(255) NOT NULL,
    description TEXT NOT NULL,
    subject_type VARCHAR(255) NULL,
    subject_id UUID NULL,
    causer_type VARCHAR(255) NULL,
    causer_id VARCHAR(255) NULL,
    properties JSONB NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX audit_log_subject_index ON audit_log (subject_type, subject_id);
CREATE INDEX audit_log_causer_index ON audit_log (causer_type, causer_id);
CREATE INDEX audit_log_batch_uuid_index ON audit_log (batch_uuid);
CREATE INDEX audit_log_log_name_index ON audit_log (log_name);"#
        .to_string())
    }

    fn down(&self) -> OrmResult<String> {
        Ok("DROP TABLE IF EXISTS audit_log;".to_string())
    }
}
"##;

const CREATE_AUTHENTICATION_LOG: &str = r##"//! Creates the `authentication_log` table (sign-in history).
//!
//! `user_id` is a `VARCHAR(255)`, not a UUID: the auth guards carry an opaque
//! string identity, so the log accepts the same value the guards use.

use rustasea::orm::Migration;
use rustasea::OrmResult;

/// `create_authentication_log_table` migration.
pub struct CreateAuthenticationLog;

impl Migration for CreateAuthenticationLog {
    fn name(&self) -> &str {
        "2027_01_01_000009_create_authentication_log_table"
    }

    fn up(&self) -> OrmResult<String> {
        Ok(r#"CREATE TABLE authentication_log (
    id UUID PRIMARY KEY,
    user_id VARCHAR(255) NULL,
    email VARCHAR(255) NULL,
    guard_name VARCHAR(255) NULL,
    event VARCHAR(64) NOT NULL,
    ip_address VARCHAR(45) NULL,
    user_agent TEXT NULL,
    successful BOOLEAN NOT NULL,
    login_at TIMESTAMPTZ NULL,
    logout_at TIMESTAMPTZ NULL,
    cleared_by_user_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX authentication_log_user_id_index ON authentication_log (user_id);
CREATE INDEX authentication_log_event_index ON authentication_log (event);
CREATE INDEX authentication_log_ip_address_index ON authentication_log (ip_address);"#
        .to_string())
    }

    fn down(&self) -> OrmResult<String> {
        Ok("DROP TABLE IF EXISTS authentication_log;".to_string())
    }
}
"##;

const FACTORIES_MOD: &str = r##"//! Model factories for tests and seeders.

pub mod user_factory;

pub use user_factory::UserFactory;
"##;

const USER_FACTORY: &str = r##"//! `UserFactory` — deterministic user fixtures.
//!
//! The default definition uses a monotonic sequence so fixtures are stable
//! across runs (`user1@example.test`, `user2@example.test`, …). For richer,
//! locale-aware data enable the umbrella crate's `faker` feature
//! (`rustasea = { version = "0.1", features = ["faker"] }`) and seed a
//! [`Faker`](rustasea::testing::faker::Faker) for reproducible randomness:
//!
//! ```ignore
//! use rustasea::testing::faker::Faker;
//!
//! // Deterministic: the same seed replays the same sequence every run.
//! let mut faker = Faker::from_config(42, "en_US");
//! let name = faker.name();
//! let email = faker.unique_email().expect("unique email");
//! ```

use rustasea::orm::Factory;
use uuid::Uuid;

use crate::app::models::User;

/// Produces `User` instances with sequence-unique emails.
#[derive(Default)]
pub struct UserFactory {
    count: usize,
}

impl Factory<User> for UserFactory {
    fn definition(&mut self) -> User {
        self.count += 1;
        User {
            id: Uuid::new_v4(),
            name: format!("User {}", self.count),
            email: format!("user{}@example.test", self.count),
            password: "hashed-placeholder".to_string(),
            email_verified_at: None,
            two_factor_secret: None,
            two_factor_recovery_codes: None,
            two_factor_confirmed_at: None,
            remember_token: None,
            timezone: None,
            deleted_at: None,
            timestamps: Default::default(),
        }
    }

    fn count(&self) -> usize {
        self.count
    }
}
"##;

const SEEDERS_MOD: &str = r##"//! Database seeders.

pub mod database_seeder;

pub use database_seeder::DatabaseSeeder;
"##;

const DATABASE_SEEDER: &str = r##"//! Seeds the default application data.

use rustasea::orm::{Result as OrmResult, Seeder};

/// `DatabaseSeeder` — inserts the default application records.
pub struct DatabaseSeeder;

impl Seeder for DatabaseSeeder {
    fn name(&self) -> &str {
        "DatabaseSeeder"
    }

    fn sql(&self) -> OrmResult<String> {
        // Seed statements must be idempotent (`ON CONFLICT DO NOTHING`).
        Ok(String::new())
    }
}
"##;
