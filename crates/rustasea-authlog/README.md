# rustasea-authlog

RustaSea authentication log: login/logout/failed/lockout history with optional new-device notification (rappasoft/laravel-authentication-log parity).

Part of the [RustaSea framework](https://github.com/rustasea/framework) - a Laravel-inspired developer experience for Rust.

## Usage

```toml
[dependencies]
rustasea-authlog = "0.1"
```

```rust
use std::sync::Arc;
use rustasea_authlog::AuthenticationLogLogger;

rustasea_authlog::install(Arc::new(AuthenticationLogLogger::new(pool.clone())));
let rows = AuthenticationLogLogger::new(pool.clone()).for_user("user-1").await?;
```

## License

MIT - see [LICENSE-MIT](https://github.com/rustasea/framework/blob/master/LICENSE-MIT).
