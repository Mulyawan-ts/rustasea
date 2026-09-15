//! Parity + adapter behaviour tests for `rustasea-action` (ADOPT-028).
//!
//! Hermetic: no network, no external services. The queue case uses a
//! file-backed sqlite database so the real `database` driver exercises job
//! serialization; the event case registers a listener for an event type unique
//! to this test binary so the process-wide registry never collides.
//!
//! Every adapter must run the same `validate → authorize → handle` pipeline;
//! these tests assert the observable parity (same output) and the mapping
//! (400/422/403), plus the validate-first ordering.

use serde::{Deserialize, Serialize};
#[cfg(any(feature = "http", feature = "queue", feature = "events"))]
use std::sync::atomic::{AtomicBool, Ordering};

use rustasea_action::{Action, ActionCommand};
#[cfg(any(feature = "http", feature = "queue", feature = "events"))]
use rustasea_validation::ErrorBag;

/// Records whether an action's `handle` body ran (short-circuit assertions).
#[cfg(any(feature = "http", feature = "queue", feature = "events"))]
static RECORDED: AtomicBool = AtomicBool::new(false);

/// Serializes tests that touch [`RECORDED`]; async-aware so a guard may be held
/// across an adapter `.await` (a std guard would trip `await_holding_lock`).
#[cfg(any(feature = "http", feature = "queue", feature = "events"))]
static RECORDING_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Doubles its input — the canonical parity action.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct DoubleAction;

#[rustasea_action::async_trait]
impl Action for DoubleAction {
    type Input = u32;
    type Output = u32;
    type Error = std::convert::Infallible;

    async fn handle(&self, input: Self::Input) -> std::result::Result<Self::Output, Self::Error> {
        Ok(input * 2)
    }
}

/// Records that it ran and rejects `0` in `validate` — the short-circuit probe.
#[cfg(any(feature = "http", feature = "queue", feature = "events"))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct RecordingAction;

#[cfg(any(feature = "http", feature = "queue", feature = "events"))]
#[rustasea_action::async_trait]
impl Action for RecordingAction {
    type Input = u32;
    type Output = u32;
    type Error = std::convert::Infallible;

    fn validate(&self, input: &Self::Input) -> std::result::Result<(), ErrorBag> {
        if *input == 0 {
            let mut bag = ErrorBag::new();
            bag.add_message("input", "input must be non-zero");
            return Err(bag);
        }
        Ok(())
    }

    async fn handle(&self, input: Self::Input) -> std::result::Result<Self::Output, Self::Error> {
        RECORDED.store(true, Ordering::SeqCst);
        Ok(input)
    }
}

/// Always refuses authorization.
#[cfg(feature = "http")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct ForbiddenAction;

#[cfg(feature = "http")]
#[rustasea_action::async_trait]
impl Action for ForbiddenAction {
    type Input = u32;
    type Output = u32;
    type Error = std::convert::Infallible;

    fn authorize(&self, _input: &Self::Input) -> bool {
        false
    }

    async fn handle(&self, input: Self::Input) -> std::result::Result<Self::Output, Self::Error> {
        Ok(input)
    }
}

/// Both rejects `0` in `validate` and refuses authorization — proves ordering.
#[cfg(feature = "http")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct ValidateAndForbidAction;

#[cfg(feature = "http")]
#[rustasea_action::async_trait]
impl Action for ValidateAndForbidAction {
    type Input = u32;
    type Output = u32;
    type Error = std::convert::Infallible;

    fn validate(&self, input: &Self::Input) -> std::result::Result<(), ErrorBag> {
        if *input == 0 {
            let mut bag = ErrorBag::new();
            bag.add_message("input", "input must be non-zero");
            return Err(bag);
        }
        Ok(())
    }

    fn authorize(&self, _input: &Self::Input) -> bool {
        false
    }

    async fn handle(&self, input: Self::Input) -> std::result::Result<Self::Output, Self::Error> {
        Ok(input)
    }
}

/// HTTP + queue + CLI produce the same result for the same input.
#[cfg(all(feature = "http", feature = "queue"))]
#[tokio::test]
async fn adapters_agree_on_the_same_output() {
    use axum::http::StatusCode;
    use rustasea_action::{ActionController, ActionJob};

    let http = ActionController::new(DoubleAction).invoke(21).await;
    assert_eq!(http.status, StatusCode::OK);
    assert_eq!(http.body, serde_json::json!(42));

    let queue = ActionJob::new(DoubleAction, 21)
        .run()
        .await
        .expect("job runs");
    assert_eq!(queue, 42);

    let cli = ActionCommand::from_json(DoubleAction, "21")
        .expect("json input")
        .run()
        .await
        .expect("command runs");
    assert_eq!(cli, 42);
}

/// The CLI adapter accepts one JSON argument and prints pretty output.
#[test]
fn command_parses_json_arguments_and_renders_output() {
    let from_args = ActionCommand::from_args(DoubleAction, &["21".to_string()])
        .expect("one json arg")
        .run_to_string();
    let rendered = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(from_args)
        .expect("renders");
    assert_eq!(rendered, "42");

    let empty = ActionCommand::from_args(DoubleAction, &[])
        .err()
        .expect("empty args rejected");
    assert!(matches!(
        empty,
        rustasea_action::ActionError::Serialization(_)
    ));

    let bad = ActionCommand::from_json(DoubleAction, "not-json")
        .err()
        .expect("bad json rejected");
    assert!(matches!(
        bad,
        rustasea_action::ActionError::Serialization(_)
    ));
}

/// HTTP mapping: malformed body → 400, validation → 422, authorization → 403.
#[cfg(feature = "http")]
#[tokio::test]
async fn http_adapter_maps_failures_to_status_codes() {
    use axum::http::StatusCode;
    use rustasea_action::ActionController;

    let malformed = ActionController::new(DoubleAction)
        .invoke_json(b"not-json")
        .await;
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);

    let invalid = ActionController::new(RecordingAction).invoke(0).await;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        invalid.body.get("errors").is_some(),
        "422 body carries errors"
    );

    let forbidden = ActionController::new(ForbiddenAction).invoke(7).await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
}

/// A validation failure short-circuits `handle` in every adapter.
#[cfg(all(feature = "http", feature = "queue"))]
#[tokio::test]
async fn validation_failure_short_circuits_handle() {
    use axum::http::StatusCode;
    use rustasea_action::{ActionController, ActionJob};

    let _guard = RECORDING_LOCK.lock().await;
    RECORDED.store(false, Ordering::SeqCst);

    let http = ActionController::new(RecordingAction).invoke(0).await;
    assert_eq!(http.status, StatusCode::UNPROCESSABLE_ENTITY);

    let queue = ActionJob::new(RecordingAction, 0)
        .run()
        .await
        .expect_err("validation fails");
    assert!(queue.is_validation());

    let cli = ActionCommand::from_json(RecordingAction, "0")
        .expect("json input")
        .run()
        .await
        .expect_err("validation fails");
    assert!(cli.is_validation());

    assert!(
        !RECORDED.load(Ordering::SeqCst),
        "handle must not run when validate rejects the input"
    );
}

/// Validate runs before authorize: an input that fails both yields 422, not 403.
#[cfg(feature = "http")]
#[tokio::test]
async fn validate_wins_over_authorize() {
    use axum::http::StatusCode;
    use rustasea_action::ActionController;

    let outcome = ActionController::new(ValidateAndForbidAction)
        .invoke(0)
        .await;
    assert_eq!(
        outcome.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "validate must run first, so a doubly-failing input is a 422"
    );
}

/// A job round-trips through JSON and still yields the same output.
#[cfg(feature = "queue")]
#[test]
fn job_round_trips_through_json() {
    use rustasea_action::ActionJob;

    let job = ActionJob::new(DoubleAction, 21);
    let encoded = serde_json::to_string(&job).expect("serialize job");
    let decoded: ActionJob<DoubleAction> = serde_json::from_str(&encoded).expect("deserialize job");

    let output = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(decoded.run())
        .expect("runs after round-trip");
    assert_eq!(output, 42);
}

/// A job dispatches through the real file-backed sqlite database driver.
#[cfg(feature = "queue")]
#[tokio::test]
async fn job_dispatches_on_file_backed_sqlite_queue() {
    use std::sync::Arc;

    use rustasea_action::ActionJob;
    use rustasea_queue::{DatabaseDriver, JobOutcome, Queue};

    let db_path = std::env::temp_dir().join(format!(
        "rustasea-action-queue-{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&db_path);

    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = rustasea_orm::DbPool::connect_with_settings(
        &url,
        rustasea_orm::PoolSettings {
            max_connections: 1,
            ..rustasea_orm::PoolSettings::default()
        },
    )
    .await
    .expect("sqlite pool");
    rustasea_queue::queue_migrator()
        .run(&pool)
        .await
        .expect("queue migrations");

    let driver = Arc::new(DatabaseDriver::new(pool));
    Queue::register_driver("database", driver);
    // Route registration is process-global; tolerate a duplicate from a rerun.
    let _ = Queue::route::<ActionJob<DoubleAction>>("database", "actions");

    // Enqueue through the real database driver (proves job serialization).
    let enqueued = Queue::dispatch(ActionJob::new(DoubleAction, 21)).await;
    assert!(
        enqueued.is_ok(),
        "enqueue through the database driver: {enqueued:?}"
    );

    // The sync connection runs inline and yields a success outcome.
    let outcome = Queue::dispatch_sync(ActionJob::new(DoubleAction, 21))
        .await
        .expect("sync dispatch");
    assert!(matches!(outcome, JobOutcome::Succeeded));

    let _ = std::fs::remove_file(&db_path);
}

/// An event whose payload converts into an action input drives the listener.
#[cfg(feature = "events")]
#[tokio::test]
async fn listener_runs_the_action_from_an_event() {
    use rustasea_action::ActionListener;
    use rustasea_events::{Dispatcher, Event};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct ActionParityEvent {
        value: u32,
    }

    impl Event for ActionParityEvent {
        fn event_name(&self) -> &'static str {
            "ActionParityEvent"
        }
    }

    impl From<ActionParityEvent> for u32 {
        fn from(event: ActionParityEvent) -> u32 {
            event.value
        }
    }

    let _guard = RECORDING_LOCK.lock().await;
    RECORDED.store(false, Ordering::SeqCst);

    Dispatcher::listen::<ActionParityEvent, _>(ActionListener::new(RecordingAction));
    Dispatcher::dispatch(ActionParityEvent { value: 5 })
        .await
        .expect("listener handles the event");

    assert!(
        RECORDED.load(Ordering::SeqCst),
        "the action's handle ran in response to the event"
    );
}

/// A rejected event payload surfaces as a typed listener failure.
#[cfg(feature = "events")]
#[tokio::test]
async fn listener_maps_validation_failure_to_event_error() {
    use rustasea_action::ActionListener;
    use rustasea_events::{Dispatcher, Event};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct RejectingEvent {
        value: u32,
    }

    impl Event for RejectingEvent {
        fn event_name(&self) -> &'static str {
            "RejectingEvent"
        }
    }

    impl From<RejectingEvent> for u32 {
        fn from(event: RejectingEvent) -> u32 {
            event.value
        }
    }

    Dispatcher::listen::<RejectingEvent, _>(ActionListener::new(RecordingAction));
    let error = Dispatcher::dispatch(RejectingEvent { value: 0 })
        .await
        .expect_err("validation failure propagates");
    assert!(matches!(error, rustasea_events::EventError::Listener(_)));
}
