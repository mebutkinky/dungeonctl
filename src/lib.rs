//! This library implements the Bluetooth LE Protocols to control smart electronic toys created by DG-LAB.
//!
//! The currently implemented devices are the Coyote 3 and PawPrints.

#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![deny(missing_docs)]

mod core;
mod error;

#[cfg(feature = "coyote3")]
pub mod coyote3;
#[cfg(feature = "pawprints")]
pub mod pawprints1;
#[cfg(feature = "pawprints")]
pub mod pawprints11;

pub use btleplug;
pub use futures_signals;

pub use self::{
    core::{StateSignal, Stereo},
    error::{Error, Result},
};

#[cfg(feature = "coyote3")]
pub use self::coyote3::Coyote3;
#[cfg(feature = "pawprints")]
pub use self::{pawprints1::PawPrints as PawPrintsLegacy, pawprints11::PawPrints};

/// LED setting values accepted by the short `0x70` LED command & `0x50`
///
/// The original app exposes these as the numeric range `0..=7` without stable color labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedColor {
    /// Raw LED option `0`.
    None,
    /// Raw LED option `1`.
    Yellow,
    /// Raw LED option `2`.
    Red,
    /// Raw LED option `3`.
    Purple,
    /// Raw LED option `4`.
    Blue,
    /// Raw LED option `5`.
    LightBlue,
    /// Raw LED option `6`.
    Green,
    /// Raw LED option `7`.
    White,
}

impl From<u8> for LedColor {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Yellow,
            2 => Self::Red,
            3 => Self::Purple,
            4 => Self::Blue,
            5 => Self::LightBlue,
            6 => Self::Green,
            7 => Self::White,
            _ => unreachable!("unknown color"),
        }
    }
}

impl LedColor {
    pub(crate) fn raw(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Yellow => 1,
            Self::Red => 2,
            Self::Purple => 3,
            Self::Blue => 4,
            Self::LightBlue => 5,
            Self::Green => 6,
            Self::White => 7,
        }
    }
}

#[cfg(all(test, feature = "pawprints"))]
mod pawprints_exports_tests {
    use super::{PawPrints, PawPrintsLegacy};

    #[test]
    fn exports_current_and_legacy_pawprints_clients() {
        fn current(_: Option<PawPrints>) {}
        fn legacy(_: Option<PawPrintsLegacy>) {}

        current(None);
        legacy(None);
    }
}
