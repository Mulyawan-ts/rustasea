/// Event dispatcher — immediate and after-response dispatch.
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use rustasea_queue::{ConcreteJob, DispatchHandle};

use crate::error::{EventError, Result};
use crate::event::Event;
use crate::job::ListenerJob;
use crate::listener::Listener;

/// Global listener registry shared by `Dispatcher` facades.
///
/// Entries are erased over the concrete listener type (never over the event —
/// the map key is the event `TypeId`), so the non-object-safe `QUEUE` const on
/// `Listener` never has to live behind a vtable.
static LISTENERS: OnceLock<Mutex<ListenerMap>> = OnceLock::new();

/// Global after-response buffer shared by `EventSink`.
static SINK: OnceLock<Mutex<Vec<Box<dyn Any + Send>>>> = OnceLock::new();

/// Process-wide dispatch observer slot, initialised on first use.
///
/// A dev profiler (debug toolbar) installs an observer here to record every
/// dispatched event's name and success flag; with no observer installed the
/// dispatch path pays only a single slot read (ADOPT-009).
static OBSERVER: OnceLock<std::sync::RwLock<Option<Arc<dyn DispatchObserver>>>> = OnceLock::new();

/// Synchronous observer notified after every [`Dispatcher::dispatch`].
///
/// Implementations must be `Send + Sync` and cheap: the profiler pushes into an
/// in-memory buffer. A panic inside `observe` is not caught, so implementations
/// must not panic.
pub trait DispatchObserver: Send + Sync {
    /// Observe one completed dispatch: the event's stable name and whether it
    /// was handled without error.
    fn observe(&self, event_name: &'static str, ok: bool);
}

/// The observer slot, initialised to empty on first use.
fn observer_slot() -> &'static std::sync::RwLock<Option<Arc<dyn DispatchObserver>>> {
    OBSERVER.get_or_init(|| std::sync::RwLock::new(None))
}

/// Install `observer` as the process-wide dispatch observer.
///
/// Call once from application boot; a later call replaces the previous
/// observer. A poisoned lock is recovered rather than surfaced, so a panic in
/// another thread cannot permanently disable observation.
pub fn register_dispatch_observer(observer: Arc<dyn DispatchObserver>) {
    let slot = observer_slot();
    let mut guard = slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = Some(observer);
}

/// Remove the process-wide dispatch observer (bootstrap/test reset hook).
pub fn clear_dispatch_observer() {
    let slot = observer_slot();
    let mut guard = slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = None;
}

/// The installed dispatch observer, or `None` when none is registered.
///
/// The dispatch path checks this after each call and returns immediately when it
/// is `None`, so an uninstrumented build pays no bookkeeping cost.
pub fn dispatch_observer() -> Option<Arc<dyn DispatchObserver>> {
    let slot = observer_slot();
    let guard = slot
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.clone()
}

/// Access the global listener map.
fn listeners() -> &'static Mutex<ListenerMap> {
    LISTENERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Object-safe listener dispatch entry.
#[async_trait]
trait ErasedListener: Send + Sync {
    /// Stable key identifying the concrete listener type.
    ///
    /// A queue-backed job stores this key so the worker can resolve the exact
    /// listener instance registered at boot.
    fn listener_key(&self) -> &'static str;

    /// Handle `event` inline, bypassing the queue configuration.
    async fn invoke_inline(&self, event: &(dyn Any + Sync)) -> Result<()>;

    /// Handle `event`, honouring the listener's queue config.
    async fn invoke_erased(&self, event: &(dyn Any + Sync)) -> Result<()>;
}

/// Erased listener map keyed by event `TypeId` (see [`LISTENERS`]).
type ListenerMap = HashMap<TypeId, Vec<Arc<dyn ErasedListener>>>;

/// Concrete listener stored behind `ErasedListener`.
struct TypedListener<E: Event, L: Listener<E>> {
    inner: L,
    _marker: std::marker::PhantomData<fn(E)>,
}

impl<E: Event, L: Listener<E>> TypedListener<E, L> {
    /// Wrap a concrete listener.
    fn new(inner: L) -> Self {
        Self {
            inner,
            _marker: std::marker::PhantomData,
        }
    }
}

#[async_trait]
impl<E: Event, L: Listener<E>> ErasedListener for TypedListener<E, L> {
    /// Stable key derived from the concrete listener type.
    fn listener_key(&self) -> &'static str {
        std::any::type_name::<L>()
    }

    /// Handle the event inline, ignoring the queue configuration.
    async fn invoke_inline(&self, event: &(dyn Any + Sync)) -> Result<()> {
        let typed = downcast_event::<E>(event)?;
        self.inner.handle(typed.clone()).await
    }

    /// Handle the event, honouring the listener's queue config.
    ///
    /// Inline listeners (`Queue { enable: false }`) run now; queue-backed
    /// listeners are serialized into a [`ListenerJob`] and pushed through the
    /// queue facade/driver, returning a typed [`EventError::Queue`] on failure.
    async fn invoke_erased(&self, event: &(dyn Any + Sync)) -> Result<()> {
        let config = self.inner.queue_config();
        if !config.enable {
            return self.invoke_inline(event).await;
        }
        let typed = downcast_event::<E>(event)?;
        let job = ListenerJob::new(self.listener_key(), typed.clone());
        let mut handle = DispatchHandle::new(Arc::new(ConcreteJob::new(job)));
        if let Some(connection) = config.connection {
            handle = handle.on_connection(connection);
        }
        if let Some(queue) = config.queue {
            handle = handle.on_queue(queue);
        }
        if let Some(delay) = config.delay {
            handle = handle.delay(delay);
        }
        handle.dispatch().await?;
        Ok(())
    }
}

/// Downcast an erased event to `E`, erroring on a type mismatch.
fn downcast_event<E: Event>(event: &(dyn Any + Sync)) -> Result<&E> {
    let any_ref: &dyn Any = event;
    any_ref
        .downcast_ref::<E>()
        .ok_or_else(|| EventError::Listener("listener received an event of the wrong type".into()))
}

/// Invoke the registered listener identified by `key` for `event` inline.
///
/// Used by a queued [`ListenerJob`] at worker execution time: the job carries
/// the listener's stable type key, resolved here against the process-wide
/// registry so the worker runs the exact listener registered at boot rather
/// than re-dispatching the event (which could re-enqueue it).
pub(crate) async fn invoke_listener_by_key<E: Event>(key: &str, event: E) -> Result<()> {
    let snapshot = {
        let map = listeners().lock().unwrap_or_else(|p| p.into_inner());
        map.get(&TypeId::of::<E>()).cloned().unwrap_or_default()
    };
    let listener = snapshot
        .iter()
        .find(|entry| entry.listener_key() == key)
        .ok_or_else(|| {
            EventError::Listener(format!(
                "no queued listener `{key}` registered for event {}",
                event.event_name()
            ))
        })?;
    listener.invoke_inline(&event).await
}

impl<E: Event, L: Listener<E>> std::fmt::Debug for TypedListener<E, L> {
    /// Debug rendering without the erased listener body.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypedListener").finish_non_exhaustive()
    }
}

/// The `Dispatcher` facade — `dispatch` + `dispatchAfterResponse`.
///
/// `dispatch` runs every registered listener for the event's type: inline when
/// the listener's queue is disabled, enqueued as a queue job when enabled.
/// `dispatch_after_response` appends to the response-flush sink so the event
/// fires only after the HTTP response has been sent.
pub struct Dispatcher;

impl Dispatcher {
    /// Register a listener for `E` (boot-time).
    ///
    /// A queue-enabled listener also registers its [`ListenerJob`] kind with
    /// the worker handler registry, so `queue:work` can resolve and execute it.
    pub fn listen<E: Event, L: Listener<E>>(listener: L) {
        if listener.queue_config().enable {
            crate::job::register_listener_job::<E>();
        }
        let mut map = listeners().lock().unwrap_or_else(|p| p.into_inner());
        map.entry(TypeId::of::<E>())
            .or_default()
            .push(Arc::new(TypedListener::<E, L>::new(listener)) as Arc<dyn ErasedListener>);
    }

    /// Dispatch `event` to all registered listeners now.
    ///
    /// When a [`DispatchObserver`] is installed it is notified once with the
    /// event's stable name and whether dispatch succeeded; with none installed
    /// the call is unchanged. The observer is looked up **after** the inner
    /// dispatch so it cannot observe an event that never ran.
    pub async fn dispatch<E: Event>(event: E) -> Result<()> {
        let name = event.event_name();
        let result = Self::dispatch_inner(event).await;
        if let Some(observer) = dispatch_observer() {
            observer.observe(name, result.is_ok());
        }
        result
    }

    /// The unobserved dispatch body (listeners run inline or enqueued).
    async fn dispatch_inner<E: Event>(event: E) -> Result<()> {
        let type_id = TypeId::of::<E>();
        let snapshot = {
            let map = listeners().lock().unwrap_or_else(|p| p.into_inner());
            map.get(&type_id).cloned().unwrap_or_default()
        };
        if snapshot.is_empty() {
            return Err(EventError::Unhandled(event.event_name()));
        }
        for erased in &snapshot {
            erased.invoke_erased(&event).await?;
        }
        Ok(())
    }

    /// Buffer `event` until the HTTP response is flushed (`EventSink`).
    pub async fn dispatch_after_response<E: Event>(event: E) -> Result<()> {
        EventSink::push(Box::new(event));
        Ok(())
    }
}

/// Test spy / response-flush sink for `dispatchAfterResponse`.
///
/// Buffered events are drained by the HTTP layer after the response is sent;
/// tests drain explicitly to assert FIFO ordering (US-M4-04 spy).
pub struct EventSink;

impl EventSink {
    /// Buffer an erased event (static storage).
    pub fn push(event: Box<dyn Any + Send>) {
        let mut buf = Self::global().lock().unwrap_or_else(|p| p.into_inner());
        buf.push(event);
    }

    /// Drain buffered events in FIFO order.
    pub fn drain() -> Vec<Box<dyn Any + Send>> {
        let mut buf = Self::global().lock().unwrap_or_else(|p| p.into_inner());
        std::mem::take(&mut *buf)
    }

    /// Number of buffered events.
    pub fn len() -> usize {
        let buf = Self::global().lock().unwrap_or_else(|p| p.into_inner());
        buf.len()
    }

    /// Whether the sink is empty.
    pub fn is_empty() -> bool {
        Self::len() == 0
    }

    /// Drain and downcast every buffered event of `T`, preserving FIFO order.
    pub fn drain_typed<T: Event + 'static>() -> Vec<T> {
        let mut out = Vec::new();
        for item in Self::drain() {
            if let Ok(typed) = item.downcast::<T>() {
                out.push(*typed);
            }
        }
        out
    }

    /// Global buffer accessor.
    fn global() -> &'static Mutex<Vec<Box<dyn Any + Send>>> {
        SINK.get_or_init(|| Mutex::new(Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Serializes tests that install the process-wide observer slot.
    ///
    /// An async-aware mutex is used because the dispatch test holds the guard
    /// across the router's `.await` (a std guard would trip `await_holding_lock`).
    static OBSERVER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// An event unique to this test module so the shared listener registry does
    /// not collide with other tests' event types.
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct ObservedEvent;

    impl Event for ObservedEvent {
        fn event_name(&self) -> &'static str {
            "ObservedEvent"
        }
    }

    /// A listener that records how many times it handled the event.
    struct CountingListener(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait]
    impl Listener<ObservedEvent> for CountingListener {
        async fn handle(&self, _event: ObservedEvent) -> Result<()> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    /// Captures observed `(name, ok)` pairs.
    #[derive(Default)]
    struct CapturingObserver {
        seen: Mutex<Vec<(&'static str, bool)>>,
    }

    impl DispatchObserver for CapturingObserver {
        fn observe(&self, event_name: &'static str, ok: bool) {
            self.seen.lock().unwrap().push((event_name, ok));
        }
    }

    /// Verifies the observer registry round-trips an installed observer.
    #[tokio::test]
    async fn observer_registry_round_trips() {
        let _guard = OBSERVER_LOCK.lock().await;
        struct Noop;
        impl DispatchObserver for Noop {
            fn observe(&self, _event_name: &'static str, _ok: bool) {}
        }
        clear_dispatch_observer();
        assert!(dispatch_observer().is_none());
        register_dispatch_observer(Arc::new(Noop));
        assert!(dispatch_observer().is_some());
        clear_dispatch_observer();
        assert!(dispatch_observer().is_none());
    }

    /// Verifies an installed observer sees the dispatched name and success flag.
    #[tokio::test]
    async fn observer_sees_name_and_ok_flag() {
        let _guard = OBSERVER_LOCK.lock().await;
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        Dispatcher::listen::<ObservedEvent, _>(CountingListener(Arc::clone(&runs)));
        let observer = Arc::new(CapturingObserver::default());
        register_dispatch_observer(observer.clone());

        Dispatcher::dispatch(ObservedEvent).await.unwrap();
        clear_dispatch_observer();

        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
        let seen = observer.seen.lock().unwrap();
        assert_eq!(seen.as_slice(), &[("ObservedEvent", true)]);
    }
}
