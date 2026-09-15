//! Global storage slot used by the export `store` terminal and the queued job.
//!
//! The crate follows the framework's process-wide-slot pattern (ADR-0007):
//! [`Excel::set_storage`](crate::Excel::set_storage) installs one
//! `Arc<dyn Storage>` that [`store`](crate::ExportBuilder::store) and
//! [`ExportJob`](crate::ExportJob) read from. Tests clear it with
//! [`Excel::clear_storage`](crate::Excel::clear_storage) so runs stay isolated.

use std::sync::{Arc, RwLock};

use rustasea_storage::Storage;

use crate::error::{ExcelError, Result};

/// The process-wide storage disk, if one has been installed.
static STORAGE: RwLock<Option<Arc<dyn Storage>>> = RwLock::new(None);

/// Install `storage` as the process-wide disk.
///
/// Returns `false` when a disk was already installed (the first install wins).
pub(crate) fn set(storage: Arc<dyn Storage>) -> bool {
    let mut slot = match STORAGE.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if slot.is_some() {
        return false;
    }
    *slot = Some(storage);
    true
}

/// Remove the installed disk (test helper; production code never clears it).
pub(crate) fn clear() {
    let mut slot = match STORAGE.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *slot = None;
}

/// Return the installed disk or a typed [`ExcelError::NotConfigured`].
pub(crate) fn get() -> Result<Arc<dyn Storage>> {
    let slot = match STORAGE.read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    slot.clone()
        .ok_or_else(|| ExcelError::NotConfigured("storage (call Excel::set_storage)".to_string()))
}

/// Write `bytes` to `key` on the installed disk.
///
/// Awaits the async [`Storage`] write directly; used by
/// [`ExportBuilder::store_async`](crate::ExportBuilder::store_async) and the
/// queued [`ExportJob`](crate::ExportJob).
pub(crate) async fn put_bytes_async(key: &str, bytes: &[u8]) -> Result<()> {
    let storage = get()?;
    storage
        .put(key, bytes)
        .await
        .map_err(|e| ExcelError::Storage {
            key: key.to_string(),
            message: e.to_string(),
        })
}

/// Synchronously write `bytes` to `key` on the installed disk.
///
/// Bridges the async [`Storage`] trait for the synchronous export `store`
/// terminal. The strategy depends on the ambient Tokio context so it never
/// panics and never deadlocks:
///
/// * **No runtime** — a private current-thread runtime drives the write.
/// * **Multi-thread runtime** — [`tokio::task::block_in_place`] hands the worker
///   off to a replacement thread and the current handle drives the write, so
///   other tasks keep making progress.
/// * **Current-thread runtime** — `block_in_place` panics there, and blocking
///   the sole runtime thread would deadlock the write, so it runs on a
///   dedicated thread with its own runtime.
///
/// The call blocks the calling thread for the duration of the write; inside
/// async code prefer [`put_bytes_async`] via
/// [`ExportBuilder::store_async`](crate::ExportBuilder::store_async) or a queued
/// [`ExportJob`](crate::ExportJob).
pub(crate) fn put_bytes(key: &str, bytes: &[u8]) -> Result<()> {
    let storage = get()?;
    let future = storage.put(key, bytes);
    let outcome = drive_blocking(future).map_err(|e| ExcelError::Storage {
        key: key.to_string(),
        message: e.to_string(),
    })?;
    outcome.map_err(|e| ExcelError::Storage {
        key: key.to_string(),
        message: e.to_string(),
    })
}

/// Drive `future` to completion from synchronous code, honouring the ambient
/// Tokio runtime (see [`put_bytes`] for the per-flavour strategy).
fn drive_blocking<F, T>(future: F) -> std::io::Result<T>
where
    F: std::future::Future<Output = T> + Send,
    T: Send,
{
    use tokio::runtime::{Builder, Handle, RuntimeFlavor};

    match Handle::try_current() {
        Ok(handle) => match handle.runtime_flavor() {
            RuntimeFlavor::MultiThread => {
                Ok(tokio::task::block_in_place(|| handle.block_on(future)))
            }
            // Current-thread (or any future flavour): a single driver thread
            // must not be blocked, so run the write on its own thread.
            _ => std::thread::scope(|scope| {
                scope
                    .spawn(move || {
                        let runtime = Builder::new_current_thread().enable_all().build()?;
                        Ok(runtime.block_on(future))
                    })
                    .join()
                    .map_err(|_| std::io::Error::other("export storage worker thread panicked"))?
            }),
        },
        Err(_) => {
            let runtime = Builder::new_current_thread().enable_all().build()?;
            Ok(runtime.block_on(future))
        }
    }
}
