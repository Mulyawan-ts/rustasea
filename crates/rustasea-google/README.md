# rustasea-google

RustaSea Google: service-account auth and cached OAuth2 access tokens (google/auth parity).

Part of the [RustaSea framework](https://github.com/rustasea/framework) - a Laravel-inspired developer experience for Rust.

## Usage

```toml
[dependencies]
rustasea-google = "0.1"
```

```rust
use rustasea_google::{GoogleAuthClient, ServiceAccount};

let account = ServiceAccount::from_file("/etc/credentials.json")?;
let client = GoogleAuthClient::new(account, scopes, None)?;
let token = client.token().await?;
```

## License

MIT - see [LICENSE-MIT](https://github.com/rustasea/framework/blob/master/LICENSE-MIT).
