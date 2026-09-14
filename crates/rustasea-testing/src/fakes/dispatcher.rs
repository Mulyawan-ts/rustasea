//! `FakeDispatcher` — the `Event::fake()` equivalent.
//!
//! [`rustasea_events::Dispatcher`] is a zero-sized facade over a process-global
//! listener map exposed only through static methods, so there is no installable
//! seam to swap. This fake mirrors the `Dispatcher::dispatch` call shape and is
//! injected by *calling it in place of* `Dispatcher`: a handler that accepts a
//! dispatcher records into the fake instead of fanning the event out to real
//! listeners. The swap is mechanical because the async signature matches.

use std::any::{Any, TypeId};
use std::sync::{Arc, Mutex};

use rustasea_events::{Event, EventError};

/// A single intercepted event, stored type-erased.
struct RecordedEvent {
    /// `TypeId` of the concrete event, for O(1) type filtering.
    type_id: TypeId,
    /// Fully-qualified event type name, for descriptive failures.
    type_name: &'static str,
    /// The owned event value, downcast back to `E` on read.
    value: Box<dyn Any + Send + Sync>,
}

/// Recording event dispatcher — the `Event::fake()` equivalent.
///
/// `Dispatcher` has no installable seam (it is a static facade over a global
/// listener map), so this fake is a drop-in *by call site*: a handler that
/// accepts a dispatcher records into the fake instead of fanning events out to
/// real listeners. Its [`FakeDispatcher::dispatch`] mirrors
/// `Dispatcher::dispatch`'s async signature so the swap is mechanical.
///
/// The type is cheaply cloneable (shared interior state), so one fake can be
/// handed to several collaborators and asserted from the test body.
///
/// ```no_run
/// use rustasea_testing::FakeDispatcher;
/// # use rustasea_events::Event;
/// # #[derive(Clone, serde::Serialize, serde::Deserialize)] struct UserCreated { id: u64 }
/// # impl Event for UserCreated {}
/// # async fn run() -> Result<(), rustasea_events::EventError> {
/// let events = FakeDispatcher::new();
/// events.dispatch(UserCreated { id: 7 }).await?;
/// events.assert_dispatched::<UserCreated>();
/// events.assert_dispatched_where::<UserCreated, _>(|e| e.id == 7);
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Default)]
pub struct FakeDispatcher {
    /// Intercepted events, in dispatch order.
    events: Arc<Mutex<Vec<RecordedEvent>>>,
}

impl FakeDispatcher {
    /// Create an empty recording dispatcher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `event` and return success (mirrors `Dispatcher::dispatch`).
    ///
    /// The event is never fanned out to listeners — interception is the point.
    pub async fn dispatch<E: Event>(&self, event: E) -> std::result::Result<(), EventError> {
        self.record(event);
        Ok(())
    }

    /// Record `event` without the async wrapper.
    pub fn record<E: Event>(&self, event: E) {
        let mut guard = self.events.lock().unwrap_or_else(|p| p.into_inner());
        guard.push(RecordedEvent {
            type_id: TypeId::of::<E>(),
            type_name: std::any::type_name::<E>(),
            value: Box::new(event),
        });
    }

    /// Number of recorded events of type `E`.
    pub fn count<E: Event>(&self) -> usize {
        let guard = self.events.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .iter()
            .filter(|e| e.type_id == TypeId::of::<E>())
            .count()
    }

    /// Clone every recorded event of type `E`, in dispatch order.
    pub fn dispatched<E: Event>(&self) -> Vec<E> {
        let guard = self.events.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .iter()
            .filter(|e| e.type_id == TypeId::of::<E>())
            .filter_map(|e| e.value.downcast_ref::<E>().cloned())
            .collect()
    }

    /// Assert `E` was dispatched at least once.
    ///
    /// # Panics
    ///
    /// Panics with a message listing every recorded event type when `E` was
    /// never dispatched.
    pub fn assert_dispatched<E: Event>(&self) {
        if self.count::<E>() == 0 {
            panic!(
                "expected event `{}` to have been dispatched, but it was not.\nrecorded dispatches: [{}]",
                std::any::type_name::<E>(),
                self.recorded_summary()
            );
        }
    }

    /// Assert `E` was **not** dispatched.
    ///
    /// # Panics
    ///
    /// Panics naming the dispatch count when `E` was dispatched.
    pub fn assert_not_dispatched<E: Event>(&self) {
        let times = self.count::<E>();
        if times > 0 {
            panic!(
                "expected event `{}` NOT to have been dispatched, but it was dispatched {times} time(s).",
                std::any::type_name::<E>()
            );
        }
    }

    /// Assert `E` was dispatched exactly `times` times.
    ///
    /// # Panics
    ///
    /// Panics when the recorded count differs from `times`.
    pub fn assert_dispatched_times<E: Event>(&self, times: usize) {
        let actual = self.count::<E>();
        assert!(
            actual == times,
            "expected event `{}` to have been dispatched {times} time(s), but it was dispatched {actual} time(s).\nrecorded dispatches: [{}]",
            std::any::type_name::<E>(),
            self.recorded_summary()
        );
    }

    /// Assert at least one dispatched `E` satisfies `predicate`.
    ///
    /// # Panics
    ///
    /// Panics when no recorded `E` matches, listing the recorded event types.
    pub fn assert_dispatched_where<E: Event, F: Fn(&E) -> bool>(&self, predicate: F) {
        let matched = self.dispatched::<E>().iter().any(&predicate);
        if !matched {
            panic!(
                "expected a dispatched event `{}` matching the given predicate, but none did.\nrecorded dispatches: [{}]",
                std::any::type_name::<E>(),
                self.recorded_summary()
            );
        }
    }

    /// Drop every recorded event.
    pub fn clear(&self) {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    /// Comma-separated list of recorded event type names, for failure messages.
    fn recorded_summary(&self) -> String {
        let guard = self.events.lock().unwrap_or_else(|p| p.into_inner());
        guard
            .iter()
            .map(|e| e.type_name)
            .collect::<Vec<_>>()
            .join(", ")
    }
}
