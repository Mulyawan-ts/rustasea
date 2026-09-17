# rustasea-activitylog

RustaSea activity log: model-change audit trail with causer/subject, property diffs, and batch grouping.

Part of the [RustaSea framework](https://github.com/rustasea/framework) - a Laravel-inspired developer experience for Rust.

## Usage

```toml
[dependencies]
rustasea-activitylog = "0.1"
```

```rust
use std::sync::Arc;
use rustasea_activitylog::ActivityLogger;

// Install once at boot:
rustasea_activitylog::install(Arc::new(ActivityLogger::new(pool.clone())));
let rows = ActivityLogger::new(pool.clone()).for_subject("User", user_id).await?;
```

## License

MIT - see [LICENSE-MIT](https://github.com/rustasea/framework/blob/master/LICENSE-MIT).
