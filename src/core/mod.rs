mod peripheral;
mod state;
mod stereo;

trait Sealed {}

pub(crate) use self::{peripheral::PeripheralExt, state::DeviceState};
pub use self::{state::StateSignal, stereo::Stereo};
