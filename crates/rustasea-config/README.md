# rustasea-config

RustaSea layered TOML configuration loader with env overlay.

Part of the [RustaSea framework](https://github.com/rustasea/framework) - a Laravel-inspired developer experience for Rust.

## Usage

```toml
[dependencies]
rustasea-config = "0.1"
```

```rust
use rustasea_config::ConfigLoader;

// Discovers every config/*.toml and applies the environment overlay on top.
let loader = ConfigLoader::load()?;
```

## License

MIT - see [LICENSE-MIT](https://github.com/rustasea/framework/blob/master/LICENSE-MIT).
