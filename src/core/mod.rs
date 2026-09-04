mod peripheral;
mod state;
mod stereo;

trait Sealed {}

pub(crate) use self::{peripheral::PeripheralExt, state::DeviceState, state::StatePublisher};
pub use self::{state::StateSignal, stereo::Stereo};
