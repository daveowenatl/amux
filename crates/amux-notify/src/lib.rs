//! Notification system for amux: types, flash animation, and notification store.
//!
//! Provides the data types for agent notifications and status,
//! the double-pulse flash animation matching cmux, and the central
//! `NotificationStore` that manages notification lifecycle.

mod flash;
mod store;
mod types;

pub use flash::{flash_alpha, FLASH_DURATION};
pub use store::NotificationStore;
pub use types::*;
