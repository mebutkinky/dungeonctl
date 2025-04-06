//! This library implements the Bluetooth LE Protocols to control smart electronic toys created by DG-LAB.
//!
//! Only the Coyote 3 is supported at the moment. Support for PawPrints is planned for the future,
//! waiting for their protocol to be stabilized/documented.

#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![deny(missing_docs)]

mod core;
mod error;

#[cfg(feature = "coyote3")]
pub mod coyote3;
// #[cfg(feature = "pawprints")]
// pub mod pawprints;

pub use btleplug;
pub use futures_signals;

pub use self::{
    core::{StateSignal, Stereo},
    error::{Error, Result},
};

#[cfg(feature = "coyote3")]
pub use self::coyote3::Coyote3;
