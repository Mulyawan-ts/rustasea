# rustasea-foundation

RustaSea application kernel: container, service providers with DAG boot, and graceful shutdown.

Part of the [RustaSea framework](https://github.com/rustasea/framework) - a Laravel-inspired developer experience for Rust.

## Usage

```toml
[dependencies]
rustasea-foundation = "0.1"
```

```rust
use rustasea_foundation::{Application, ServiceProvider};

// Providers register first, then boot, in dependency order.
let app = Application::new();
app.register(provider);
app.boot()?;
```

## License

MIT - see [LICENSE-MIT](https://github.com/rustasea/framework/blob/master/LICENSE-MIT).
