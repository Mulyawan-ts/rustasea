//! Manager tests: connection registry, config resolution, env bridge, and the
//! process-wide slots (ADOPT-022).

use std::sync::Arc;

use super::{clear, config, gate, manager, set_config, set_manager};
use super::{BroadcastManager, BroadcastingConfig, DEFAULT_CONNECTION};
use crate::broadcaster::BroadcastPayload;
use crate::channel::{AuthDecision, Authorize, Channel, Subscriber};
use crate::error::BroadcastError;

/// Serializes tests that read/write the process environment.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Restore an environment variable to its prior value.
fn restore(key: &str, prior: Option<String>) {
    match prior {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}

/// A default config resolves to the in-process hub.
#[tokio::test]
async fn default_config_builds_hub() {
    let config = BroadcastingConfig::default();
    let manager = BroadcastManager::from_config(&config, None).expect("hub builds");
    assert_eq!(manager.default_name(), "hub");
    assert_eq!(manager.default_connection().unwrap().connection(), "hub");
}

/// Publishing through the default hub connection succeeds and is delivered.
#[tokio::test]
async fn default_hub_publish_delivers() {
    let config = BroadcastingConfig::default();
    let manager = BroadcastManager::from_config(&config, None).expect("hub builds");
    let payload = BroadcastPayload::new("Ping", "orders.1", serde_json::json!({ "n": 1 }));
    manager.publish(payload).await.expect("publish");
}

/// An unknown connection name is a typed error.
#[tokio::test]
async fn unknown_connection_is_typed_error() {
    let manager = BroadcastManager::new("hub", None);
    let err = match manager.connection("nope") {
        Ok(_) => panic!("an unknown connection must not resolve"),
        Err(error) => error,
    };
    assert!(matches!(
        err,
        BroadcastError::ConnectionUnknown { ref connection } if connection == "nope"
    ));
}

/// A default naming an unbuilt driver is `NotConfigured`.
#[tokio::test]
async fn missing_default_driver_is_not_configured() {
    let config = BroadcastingConfig {
        default: "ably".to_string(),
        ..BroadcastingConfig::default()
    };
    let err = match BroadcastManager::from_config(&config, None) {
        Ok(_) => panic!("an unbuilt default driver must not build"),
        Err(error) => error,
    };
    assert!(matches!(
        err,
        BroadcastError::NotConfigured { ref connection } if connection == "ably"
    ));
}

/// `BROADCAST_CONNECTION` overrides the default selector.
#[test]
fn broadcast_connection_env_overrides_default() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prior = std::env::var("BROADCAST_CONNECTION").ok();
    std::env::set_var("BROADCAST_CONNECTION", "pusher");

    let mut config = BroadcastingConfig::default();
    config.apply_env();
    assert_eq!(config.default, "pusher");

    restore("BROADCAST_CONNECTION", prior);
}

/// A blank `BROADCAST_CONNECTION` is ignored (keeps the file/default value).
#[test]
fn blank_broadcast_connection_is_ignored() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prior = std::env::var("BROADCAST_CONNECTION").ok();
    std::env::set_var("BROADCAST_CONNECTION", "   ");

    let mut config = BroadcastingConfig::default();
    config.apply_env();
    assert_eq!(config.default, DEFAULT_CONNECTION);

    restore("BROADCAST_CONNECTION", prior);
}

/// A private channel with no gate fails closed as unauthenticated.
#[tokio::test]
async fn private_channel_without_gate_is_unauthenticated() {
    let config = BroadcastingConfig::default();
    let manager = BroadcastManager::from_config(&config, None).expect("hub builds");
    let subscriber = Subscriber {
        id: "user-1".to_string(),
        name: None,
    };
    let err = manager
        .authorize("private-chat.1", &subscriber)
        .await
        .unwrap_err();
    assert!(matches!(err, BroadcastError::Unauthenticated { .. }));
}

/// A public channel authorizes without a gate.
#[tokio::test]
async fn public_channel_authorizes_without_gate() {
    let config = BroadcastingConfig::default();
    let manager = BroadcastManager::from_config(&config, None).expect("hub builds");
    let subscriber = Subscriber {
        id: "user-1".to_string(),
        name: None,
    };
    manager
        .authorize("orders.1", &subscriber)
        .await
        .expect("public channel needs no gate");
}

/// A broadcastable event dispatches through the default connection.
#[tokio::test]
async fn dispatch_serializes_and_publishes() {
    struct UserCreated {
        name: String,
    }
    impl crate::ShouldBroadcast for UserCreated {
        fn broadcast_on(&self) -> Channel {
            Channel::Private("chat.1".to_string())
        }
    }
    impl crate::BroadcastEvent for UserCreated {
        fn event_name(&self) -> &'static str {
            "UserCreated"
        }
    }
    impl serde::Serialize for UserCreated {
        fn serialize<S: serde::Serializer>(
            &self,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            serde::Serialize::serialize(&serde_json::json!({ "name": self.name }), serializer)
        }
    }

    let config = BroadcastingConfig::default();
    let manager = BroadcastManager::from_config(&config, None).expect("hub builds");
    manager
        .dispatch(&UserCreated {
            name: "Ada".to_string(),
        })
        .await
        .expect("dispatch");
}

/// The process-wide config/gate/manager slots install and clear.
#[test]
fn global_slots_install_and_clear() {
    clear();
    assert!(config().is_none());
    assert!(gate().is_none());
    assert!(manager().is_none());

    set_config(BroadcastingConfig::default());
    assert_eq!(config().unwrap().default, DEFAULT_CONNECTION);

    let built =
        BroadcastManager::from_config(&BroadcastingConfig::default(), None).expect("hub builds");
    set_manager(Arc::new(built));
    assert!(manager().is_some());

    clear();
    assert!(manager().is_none());
}

/// Pusher client auth signs the private channel and echoes channel_data for
/// presence.
#[cfg(feature = "pusher")]
#[tokio::test]
async fn pusher_channel_auth_signs_private_and_presence() {
    use crate::pusher::PusherConfig;

    struct AllowAll;
    #[async_trait::async_trait]
    impl Authorize for AllowAll {
        async fn authorize(&self, _channel: &str, _identity: &str) -> AuthDecision {
            AuthDecision::Allow
        }
    }

    let config = BroadcastingConfig {
        default: "pusher".to_string(),
        pusher: Some(PusherConfig {
            app_id: "1".to_string(),
            key: "key".to_string(),
            secret: "secret".to_string(),
            cluster: Some("us2".to_string()),
            ..PusherConfig::default()
        }),
        ..BroadcastingConfig::default()
    };
    let manager = BroadcastManager::from_config(&config, Some(Arc::new(AllowAll))).expect("builds");
    let subscriber = Subscriber {
        id: "user-1".to_string(),
        name: None,
    };

    let private = manager
        .pusher_channel_auth("123.456", "private-chat.1", &subscriber, None)
        .await
        .expect("private auth");
    assert!(private.auth.starts_with("key:"));
    assert_eq!(private.channel_data, None);

    let presence = manager
        .pusher_channel_auth(
            "123.456",
            "presence-chat.1",
            &subscriber,
            Some("{\"user_id\":\"1\"}"),
        )
        .await
        .expect("presence auth");
    assert_eq!(
        presence.channel_data.as_deref(),
        Some("{\"user_id\":\"1\"}")
    );
}
