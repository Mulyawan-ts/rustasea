# rustasea

RustaSea umbrella crate. Laravel-inspired developer experience for Rust with typed re-exports of the framework.

Part of the [RustaSea framework](https://github.com/rustasea/framework) - a Laravel-inspired developer experience for Rust.

The facade re-exports the framework crates (foundation, config, router, orm, auth, queue, and more) under short module names such as `rustasea::orm` and `rustasea::auth`, and gates the optional layers behind cargo features.

## Usage

```toml
[dependencies]
rustasea = { version = "0.1", features = ["view"] }
```

## License

MIT - see [LICENSE-MIT](https://github.com/rustasea/framework/blob/master/LICENSE-MIT).
