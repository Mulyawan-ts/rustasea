# rustasea-timezone

RustaSea timezone: IANA validation, user-timezone resolution chain, and UTC/local formatting helpers.

Part of the [RustaSea framework](https://github.com/rustasea/framework) - a Laravel-inspired developer experience for Rust.

## Usage

```toml
[dependencies]
rustasea-timezone = "0.1"
```

```rust
use rustasea_timezone::{resolve, validate, format_local};

// User preference wins; an invalid candidate falls through.
assert_eq!(resolve(Some("Asia/Jakarta"), None, None, "UTC"), "Asia/Jakarta");
```

## License

MIT - see [LICENSE-MIT](https://github.com/rustasea/framework/blob/master/LICENSE-MIT).
