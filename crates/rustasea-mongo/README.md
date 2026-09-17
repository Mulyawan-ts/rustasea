# rustasea-mongo

RustaSea Mongo: MongoDB document store with a typed client, Document contract, CRUD helpers, and filter/update builders.

Part of the [RustaSea framework](https://github.com/rustasea/framework) - a Laravel-inspired developer experience for Rust.

## Usage

```toml
[dependencies]
rustasea-mongo = "0.1"
```

```rust
use rustasea_mongo::{Document, Filter, MongoClient, MongoConfig};

// A serde type implements Document to bind to a collection.
let client = MongoClient::connect(&config).await?;
```

## License

MIT - see [LICENSE-MIT](https://github.com/rustasea/framework/blob/master/LICENSE-MIT).
