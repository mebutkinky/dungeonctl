//! This library implements the Bluetooth LE Protocols to control smart electronic toys created by DG-LAB.
//!
//! Only the Coyote 3 is supported at the moment.
//!
//! TODO:
//!
//! - [ ] Support PawPrints
//! - [ ] Support Civet
//! - [ ] Support Opossum

#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![warn(missing_docs)]

mod builder;
mod error;

#[cfg(feature = "coyote3")]
pub mod coyote3;
// #[cfg(feature = "pawprints")]
// pub mod pawprints;

use dungeonctl_core as core;
use dungeonctl_utils as utils;

pub use self::builder::BleDeviceConnectionBuilder;
pub use self::error::{Error, Result};

#[cfg(feature = "coyote3")]
pub use self::coyote3::Coyote3;
