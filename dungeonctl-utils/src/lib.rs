//! Common helper types used by other `dungeonctl-*` subcrates.

mod state;

trait Sealed {}

pub use futures_signals;

pub use self::state::{DeviceState, StateSignal};
