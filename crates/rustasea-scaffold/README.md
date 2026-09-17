# rustasea-scaffold

RustaSea starter-kit scaffolder: emits the shared core plus the blade/react/vue/livewire presentation tree.

Part of the [RustaSea framework](https://github.com/rustasea/framework) - a Laravel-inspired developer experience for Rust.

## Usage

```toml
[dependencies]
rustasea-scaffold = "0.1"
```

```rust
use rustasea_scaffold::{Scaffold, StarterKitVariant};

let files = Scaffold::new("my-app", StarterKitVariant::Blade)
    .render()
    .expect("render");
assert!(files.iter().any(|f| f.path == "routes/web.rs"));
```

## License

MIT - see [LICENSE-MIT](https://github.com/rustasea/framework/blob/master/LICENSE-MIT).
