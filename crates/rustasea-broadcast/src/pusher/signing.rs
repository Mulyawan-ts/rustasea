//! Pusher request signing (ADOPT-022).
//!
//! Implements the Pusher HTTP API signing scheme:
//!
//! * `body_md5` — the MD5 digest (lowercase hex) of the exact request body.
//! * `auth_signature` — `HMAC-SHA256(secret, string_to_sign)` (lowercase hex),
//!   where the string-to-sign is `POST\n{path}\n{sorted query}`.
//!
//! It also implements the client-side channel authorization signature
//! (`{key}:{hmac}`) used for `private-`/`presence-` subscriptions.
//!
//! All helpers return a typed [`BroadcastError`] instead of panicking; HMAC
//! accepts a key of any length, so a failure here is effectively impossible but
//! is still propagated rather than `unwrap`-ed.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::{BroadcastError, Result};

/// Hex-encode the HMAC-SHA256 of `message` under `secret`.
///
/// # Errors
///
/// [`BroadcastError::Transport`] if the HMAC context cannot be initialized
/// (unreachable for HMAC-SHA256, which accepts any key length).
pub(crate) fn hmac_sha256_hex(secret: &str, message: &str) -> Result<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|error| BroadcastError::Transport(format!("hmac init failed: {error}")))?;
    mac.update(message.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Lowercase hex MD5 of the request body (Pusher `body_md5`).
pub(crate) fn body_md5(body: &str) -> String {
    format!("{:x}", md5::compute(body.as_bytes()))
}

/// The Pusher string-to-sign for a `POST` to `path` with `query`.
pub(crate) fn string_to_sign(path: &str, query: &str) -> String {
    format!("POST\n{path}\n{query}")
}

/// The client-side channel authorization signature for a socket.
///
/// `private` channels sign `{socket_id}:{channel}`; `presence` channels sign
/// `{socket_id}:{channel}:{channel_data}`.
pub(crate) fn channel_auth_string(
    socket_id: &str,
    wire_channel: &str,
    channel_data: Option<&str>,
) -> String {
    match channel_data {
        Some(data) => format!("{socket_id}:{wire_channel}:{data}"),
        None => format!("{socket_id}:{wire_channel}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MD5 digest matches an independent computation.
    #[test]
    fn body_md5_matches_reference() {
        // MD5("hello") = 5d41402abc4b2a76b9719d911017c592
        assert_eq!(body_md5("hello"), "5d41402abc4b2a76b9719d911017c592");
    }

    /// The string-to-sign is the documented three-line form.
    #[test]
    fn string_to_sign_shape() {
        assert_eq!(
            string_to_sign("/apps/1/events", "auth_key=k&auth_version=1.0"),
            "POST\n/apps/1/events\nauth_key=k&auth_version=1.0"
        );
    }

    /// The channel-auth string differs for private vs presence channels.
    #[test]
    fn channel_auth_string_private_vs_presence() {
        assert_eq!(
            channel_auth_string("123.456", "private-chat.1", None),
            "123.456:private-chat.1"
        );
        assert_eq!(
            channel_auth_string("123.456", "presence-chat.1", Some("{\"user_id\":\"1\"}")),
            "123.456:presence-chat.1:{\"user_id\":\"1\"}"
        );
    }
}
