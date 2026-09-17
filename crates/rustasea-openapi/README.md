# rustasea-openapi

RustaSea OpenAPI: generate an OpenAPI 3.1 document from the live route table.

Part of the [RustaSea framework](https://github.com/rustasea/framework) - a Laravel-inspired developer experience for Rust.

## Usage

```toml
[dependencies]
rustasea-openapi = "0.1"
```

```rust
use rustasea_openapi::generate;

// The router's route table is the single source of truth.
let spec = generate(&routes).expect("valid route table");
assert_eq!(spec["openapi"], "3.1.0");
```

## License

MIT - see [LICENSE-MIT](https://github.com/rustasea/framework/blob/master/LICENSE-MIT).
