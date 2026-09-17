# rustasea-inertia-client

RustaSea Inertia WASM client: page parsing, generated component registry, and navigation.

Part of the [RustaSea framework](https://github.com/rustasea/framework) - a Laravel-inspired developer experience for Rust.

## Usage

```toml
[dependencies]
rustasea-inertia-client = "0.1"
```

```rust
use rustasea_inertia_client::{inertia_registry, ComponentRegistry, Value};

fn mount_login(_props: &Value) -> Result<(), rustasea_inertia_client::ClientError> {
    Ok(())
}
inertia_registry!(AppRegistry { "auth/login" => mount_login });
```

## License

MIT - see [LICENSE-MIT](https://github.com/rustasea/framework/blob/master/LICENSE-MIT).
