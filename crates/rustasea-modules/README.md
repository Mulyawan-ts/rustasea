# rustasea-modules

Modular application support (ADOPT-027): module manifest, deterministic registry, and the runtime Module contract (nwidart/laravel-modules parity).

Part of the [RustaSea framework](https://github.com/rustasea/framework) - a Laravel-inspired developer experience for Rust.

## Usage

```toml
[dependencies]
rustasea-modules = "0.1"
```

```rust
use rustasea_modules::{ModuleManifest, ModuleRegistry};

let manifest = ModuleManifest::read(std::path::Path::new("."))?;
let mut registry = ModuleRegistry::new();
registry.apply_manifest(&manifest);
```

## License

MIT - see [LICENSE-MIT](https://github.com/rustasea/framework/blob/master/LICENSE-MIT).
