use dungeonctl_pattern::{Mono, Stereo};

/// Represents the channel layout which is either [`Mono`] or [`Stereo`].
#[allow(private_bounds)]
pub trait ChannelLayout: self::sealed::Sealed + dungeonctl_pattern::ChannelLayout {
    #[doc(hidden)]
    type AudioBuffer<'a>: AudioBuffer<'a>;
    #[doc(hidden)]
    type Array<T>: ChannelArray<T>;
    #[doc(hidden)]
    type PatternFrame: PatternFrame<FromArray = Self::Array<dungeonctl_pattern::Sample>>;
}

mod sealed {
    pub trait Sealed {}
}

pub(crate) trait AudioBuffer<'a> {
    fn from_planes(buf: &'a [f32]) -> Self;
    fn iter_channels(&self) -> impl Iterator<Item = &'a [f32]>;
}

impl<'a> AudioBuffer<'a> for &'a [f32] {
    fn from_planes(buf: &'a [f32]) -> Self {
        buf
    }
    fn iter_channels(&self) -> impl Iterator<Item = &'a [f32]> {
        std::iter::once(*self)
    }
}

impl<'a> AudioBuffer<'a> for [&'a [f32]; 2] {
    fn from_planes(buf: &'a [f32]) -> Self {
        let half_len = buf.len() / 2;
        [&buf[..half_len], &buf[half_len..]]
    }
    fn iter_channels(&self) -> impl Iterator<Item = &'a [f32]> {
        self.iter().copied()
    }
}

pub(crate) trait ChannelArray<T>: AsRef<[T]> + AsMut<[T]> {
    fn from_fn(f: impl FnMut(usize) -> T) -> Self;
}

impl<T> ChannelArray<T> for [T; 1] {
    fn from_fn(f: impl FnMut(usize) -> T) -> Self {
        std::array::from_fn(f)
    }
}

impl<T> ChannelArray<T> for [T; 2] {
    fn from_fn(f: impl FnMut(usize) -> T) -> Self {
        std::array::from_fn(f)
    }
}

pub(crate) trait PatternFrame {
    type FromArray;

    fn from_array(array: Self::FromArray) -> Self;
}

impl PatternFrame for dungeonctl_pattern::Sample {
    type FromArray = [dungeonctl_pattern::Sample; 1];

    fn from_array([sample]: Self::FromArray) -> Self {
        sample
    }
}

impl PatternFrame for dungeonctl_pattern::StereoFrame {
    type FromArray = [dungeonctl_pattern::Sample; 2];

    fn from_array([left, right]: Self::FromArray) -> Self {
        Self { a: left, b: right }
    }
}

impl ChannelLayout for Mono {
    type AudioBuffer<'a> = &'a [f32];
    type Array<T> = [T; 1];
    type PatternFrame = dungeonctl_pattern::Sample;
}
impl self::sealed::Sealed for Mono {}

impl ChannelLayout for Stereo {
    type AudioBuffer<'a> = [&'a [f32]; 2];
    type Array<T> = [T; 2];
    type PatternFrame = dungeonctl_pattern::StereoFrame;
}
impl self::sealed::Sealed for Stereo {}
