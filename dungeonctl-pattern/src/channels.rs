use crate::{Sample, StereoFrame};

/// Represents the channel layout which is either [`Mono`] or [`Stereo`].
#[allow(private_bounds)]
pub trait ChannelLayout: self::sealed::Sealed {
    #[doc(hidden)]
    const N: usize;
    #[doc(hidden)]
    type Frame;
}

mod sealed {
    pub trait Sealed {}
}

/// A channel layout with one single channel.
#[derive(Clone, Copy, Debug)]
pub struct Mono;
impl ChannelLayout for Mono {
    const N: usize = 1;
    type Frame = Sample;
}
impl self::sealed::Sealed for Mono {}

/// A channel layout with two channel s.

#[derive(Clone, Copy, Debug)]
pub struct Stereo;
impl ChannelLayout for Stereo {
    const N: usize = 2;
    type Frame = StereoFrame;
}
impl self::sealed::Sealed for Stereo {}
