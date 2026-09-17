# rustasea-i18n

RustaSea i18n: TOML/JSON translation dictionaries, interpolation, and pluralization.

Part of the [RustaSea framework](https://github.com/rustasea/framework) - a Laravel-inspired developer experience for Rust.

## Usage

```toml
[dependencies]
rustasea-i18n = "0.1"
```

```rust
use rustasea_i18n::{TranslationLoader, Translator};

let loader = TranslationLoader::default();
let translator = Translator::load(&loader, "en", "en")?;
```

## License

MIT - see [LICENSE-MIT](https://github.com/rustasea/framework/blob/master/LICENSE-MIT).
