//! Generated `config/*.toml` files.
//!
//! The config loader auto-discovers every `config/*.toml` (ADR-0002 decision 8),
//! so the starter kit ships one file per concern. The templates mirror the
//! workspace-root `config/*.toml` files that are the source of truth for the
//! Laravel 13.x parity surface (tasks CFG-001..CFG-011): `app`, `auth`, `cache`,
//! `database`, `queue`, `session`, `logging`, `mail`, `services`, `storage`,
//! `fortify`, and the standalone `mongo` connection. Long explanatory comment
//! blocks are trimmed, but every default a generated app needs to parse is
//! preserved. `inertia.toml` is emitted only for the react/vue variants.

use crate::variant::StarterKitVariant;

use super::TemplateFile;

/// Config templates for `variant`.
///
/// Emits the eleven Laravel-parity configs plus the standalone `mongo.toml`
/// surface, then appends `inertia.toml` for the Inertia variants. Order is
/// deterministic so generated trees diff cleanly.
pub fn entries(variant: StarterKitVariant) -> Vec<TemplateFile> {
    let mut files = vec![
        ("config/app.toml", APP),
        ("config/auth.toml", AUTH),
        ("config/cache.toml", CACHE),
        ("config/database.toml", DATABASE),
        ("config/queue.toml", QUEUE),
        ("config/session.toml", SESSION),
        ("config/logging.toml", LOGGING),
        ("config/mail.toml", MAIL),
        ("config/services.toml", SERVICES),
        ("config/storage.toml", STORAGE),
        ("config/fortify.toml", FORTIFY),
        ("config/mongo.toml", MONGO),
    ];
    if variant.uses_inertia() {
        files.push(("config/inertia.toml", INERTIA));
    }
    files
}

/// Application identity — flat `app_*` keys (Laravel `config/app.php`).
const APP: &str = r##"# Application configuration — mirrors Laravel 13.x config/app.php.
# Flat `app_*` keys; APP_* environment variables override these (env wins).
app_name = "@@app_name@@"
app_env = "local"
app_debug = true
app_url = "http://localhost:8000"
app_timezone = "UTC"
app_locale = "en"
app_fallback_locale = "en"
app_faker_locale = "en_US"
app_cipher = "AES-256-CBC"
# Supply the encryption key via APP_KEY; empty is treated as "unset".
app_key = ""
app_previous_keys = []
app_maintenance_driver = "file"
app_maintenance_store = "database"
"##;

/// Authentication guards, providers, and password brokers (`config/auth.php`).
const AUTH: &str = r##"# Authentication configuration — mirrors Laravel 13.x config/auth.php.
[auth]
password_timeout = 10800

[auth.defaults]
guard = "web"
passwords = "users"

[auth.guards.web]
driver = "session"
provider = "users"

[auth.guards.api]
driver = "token"
provider = "users"

[auth.providers.users]
driver = "eloquent"
model = "App\\Models\\User"

[auth.passwords.users]
provider = "users"
table = "password_reset_tokens"
expire = 60
throttle = 60
"##;

/// Cache stores (`config/cache.php`); the `-cache-` prefix marker is required.
const CACHE: &str = r##"# Cache configuration — mirrors Laravel 13.x config/cache.php.
[cache]
default = "memory"
# MUST contain the hyphenated `-cache-` marker or loading fails.
prefix = "rustasea-cache-"
serializable_classes = []

[cache.stores.memory]
driver = "memory"
serialize = false

# Feature `redis` required; url may come from REDIS_URL.
[cache.stores.redis]
driver = "redis"
connection = "default"
lock_connection = "default"
"##;

/// Named database connections plus migrations/Redis (`config/database.php`).
const DATABASE: &str = r##"# Database connections — mirrors Laravel 13.x config/database.php.
# DATABASE_URL overrides the resolved URL at runtime.
[database]
default = "sqlite"
url = "sqlite://database.sqlite?mode=rwc"
pool = { min = 1, max = 10, idle_timeout = 600 }

[database.connections.sqlite]
driver = "sqlite"
url = "sqlite://database.sqlite?mode=rwc"

[database.connections.pgsql]
driver = "postgres"
host = "127.0.0.1"
port = 5432
database = "rustasea"
username = "rustasea"
password = "secret"

[database.connections.mysql]
driver = "mysql"
host = "127.0.0.1"
port = 3306
database = "rustasea"
username = "rustasea"
password = "secret"

[database.connections.mongo]
driver = "mongodb"
uri = "mongodb://127.0.0.1:27017"
database = "rustasea"

[database.migrations]
table = "migrations"
update_date_on_publish = true

[database.redis]
client = "deadpool"

[database.redis.options]
cluster = "redis"
prefix = "rustasea-database-"
persistent = false

[database.redis.default]
url = "redis://127.0.0.1:6379/0"
database = 0

[database.redis.cache]
url = "redis://127.0.0.1:6379/1"
database = 1
"##;

/// Queue connections, batching, and failed-job storage (`config/queue.php`).
const QUEUE: &str = r##"# Queue configuration — mirrors Laravel 13.x config/queue.php.
[queue]
default = "database"

[queue.connections.sync]
driver = "sync"

[queue.connections.database]
driver = "database"
table = "jobs"
queue = "default"
retry_after = 90
after_commit = false

[queue.connections.redis]
driver = "redis"
queue = "default"
retry_after = 90

[queue.batching]
table = "job_batches"

[queue.failed]
driver = "database-uuids"
table = "failed_jobs"
"##;

/// Session driver and cookie policy (`config/session.php`).
const SESSION: &str = r##"# Session configuration — mirrors Laravel 13.x config/session.php.
# Only the `memory` driver is implemented today; other names fail closed.
[session]
driver = "memory"
lifetime = 120
expire_on_close = false
encrypt = false
files = "storage/framework/sessions"
connection = "default"
table = "sessions"
store = "default"
lottery = [2, 100]
# The `-session-` marker is required so session keys never collide with cache.
cookie = "rustasea-session"
path = "/"
domain = ""
secure = false
http_only = true
same_site = "lax"
partitioned = false
serialization = "json"
"##;

/// Default channel, deprecations, and named channels (`config/logging.php`).
const LOGGING: &str = r##"# Logging configuration — mirrors Laravel 13.x config/logging.php.
[logging]
default = "stack"

# Sentry error tracking (ADOPT-004). Requires the `sentry` cargo feature on
# `rustasea-logging`/`rustasea-app`. Leave `dsn` empty to disable Sentry.
# Environment overrides: SENTRY_DSN, SENTRY_TRACES_SAMPLE_RATE, SENTRY_ENVIRONMENT.
#
# [logging.sentry]
# dsn = ""
# traces_sample_rate = 0.0
# environment = "local"

[logging.deprecations]
channel = "null"
trace = false

[logging.channels.stack]
driver = "stack"
channels = ["daily", "stderr"]
ignore_exceptions = false

[logging.channels.single]
driver = "single"
path = "storage/logs/rustasea.log"
level = "debug"
replace_placeholders = false

[logging.channels.daily]
driver = "daily"
path = "storage/logs/rustasea.log"
level = "debug"
max_files = 14

[logging.channels.stderr]
driver = "stderr"
level = "debug"

[logging.channels.null]
driver = "null"
"##;

/// Default mailer, sender, and named mailers (`config/mail.php`).
const MAIL: &str = r##"# Mail configuration — mirrors Laravel 13.x config/mail.php.
[mail]
default = "log"

[mail.from]
address = "hello@example.com"
name = "@@app_pascal@@"

# The `smtp` transport requires the `smtp` feature; credentials via MAIL_*.
[mail.mailers.smtp]
transport = "smtp"
scheme = "smtp"
url = ""
host = "127.0.0.1"
port = 2525
username = ""
password = ""
timeout = 5
local_domain = ""

[mail.mailers.log]
transport = "log"
channel = ""

[mail.mailers.array]
transport = "array"

[mail.mailers.failover]
transport = "failover"
mailers = ["smtp", "log"]
retry_after = 60
"##;

/// Third-party credentials (`config/services.php`); never commit real secrets.
const SERVICES: &str = r##"# Third-party service credentials — mirrors Laravel 13.x config/services.php.
# Never commit real secrets; supply values through the environment instead.
[services.postmark]
key = ""

[services.resend]
key = ""

[services.ses]
key = ""
secret = ""
region = "us-east-1"

[services.slack.notifications]
bot_user_oauth_token = ""
channel = ""
"##;

/// Storage disks and symlinks (`config/filesystems.php`).
const STORAGE: &str = r##"# Storage configuration — mirrors Laravel 13.x config/filesystems.php.
[storage]
default = "local"

[storage.links]
"public/storage" = "storage/app/public"

[storage.disks.local]
driver = "local"
root = "storage/app"
serve = true
visibility = "local"
throw = false
report = false

[storage.disks.public]
driver = "local"
root = "storage/app/public"
visibility = "public"
throw = false
report = false

[storage.disks.archive]
driver = "local"
root = "storage/archive"
visibility = "local"
throw = false
report = false
"##;

/// Fortify-equivalent auth feature surface (Laravel Fortify `config/fortify.php`).
///
/// Passkeys and two-factor authentication are inert parity: the keys parse but
/// no runtime wiring consumes them yet.
const FORTIFY: &str = r##"# Fortify-equivalent auth features — mirrors Laravel Fortify config/fortify.php.
# FORTIFY_* environment variables override these (env wins).
[fortify]
guard = "web"
passwords = "users"
username = "email"
email = "email"
lowercase_usernames = true
home = "/dashboard"
prefix = ""
middleware = ["web"]
views = true

[fortify.limiters]
login = "login"

[fortify.passkeys]
# PASSKEYS_USER_HANDLE_SECRET overrides; falls back to app.key when blank.
user_handle_secret = ""
timeout = 60000

[fortify.features]
registration = true
reset_passwords = true
email_verification = true

[fortify.features.two_factor_authentication]
confirm = true
confirm_password = true

[fortify.features.passkeys]
confirm_password = true
"##;

/// Standalone MongoDB connection surface (`config/mongo.toml`).
const MONGO: &str = r##"# MongoDB connection — MONGODB_URI / MONGODB_DATABASE override at runtime.
[mongo]
uri = "mongodb://localhost:27017"
database = "rustasea"
"##;

/// Inertia asset contract for the react/vue variants.
const INERTIA: &str = r##"[inertia]
# Asset version used for cache-busting and the 409 hard-navigation flow.
version = "1"
ssr = false
"##;
