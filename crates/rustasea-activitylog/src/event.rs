//! Activity event types — re-exported from the ORM's audit seam.
//!
//! The canonical definitions live in [`rustasea_orm::activity`] so the ORM's
//! write path can build events without depending on this crate. They are
//! re-exported here for ergonomic `rustasea_activitylog::ActivityEvent` imports
//! and paired with the crate-level [`crate::ActivityLogger`].

pub use rustasea_orm::activity::{
    ActivityColumnMode, ActivityColumns, ActivityEvent, ActivityLogError, ActivityOperation,
    ActivityRecorder,
};
