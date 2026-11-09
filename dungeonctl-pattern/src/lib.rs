//! This library provides representations for e-stim patterns that can be played by the DG-LAB Coyote 3.

#![cfg_attr(docsrs, feature(doc_auto_cfg, doc_notable_trait))]
#![warn(missing_docs)]

mod channels;
#[cfg(feature = "stream")]
pub mod stream;

use std::{ops::Range, path::Path, time::Duration};

use dungeonctl_core::StereoValue;
use ndarray::{ArrayView2, ArrayView3, ArrayViewMut2, ArrayViewMut3, Axis, aview_mut2, aview2, s};
use ndarray_stats::QuantileExt;
use noisy_float::prelude::n64;

pub use self::channels::{ChannelLayout, Mono, Stereo};

#[cfg(feature = "stream")]
use self::stream::PatternStream;

/// The Coyote 3 sends pulses every 25 ms.
pub const PULSE_INTERVAL: Duration = Duration::from_millis(25);

/// Wraps an inner type where each value is in the range 0.0..=1.0.
///
/// This can be used to scale a pattern in a specific range.
#[derive(Clone, Copy, Debug, Default)]
pub struct Relative<T>(pub T);

impl<T> Relative<T> {
    /// Applies a function to the inner value.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Relative<U> {
        Relative(f(self.0))
    }
}

impl<T> AsRef<T> for Relative<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}
impl<T> AsMut<T> for Relative<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

trait IntoRelative: Sized {
    type Range;

    fn into_relative(self, _: &Self::Range) -> Relative<Self>;
}

impl IntoRelative for f32 {
    type Range = Range<f32>;

    fn into_relative(self, Range { start, end }: &Range<f32>) -> Relative<Self> {
        Relative((self.clamp(*start, *end) - start) / (end - start))
    }
}

impl IntoRelative for Sample {
    type Range = AudioPatternRange;

    fn into_relative(self, range: &AudioPatternRange) -> Relative<Self> {
        fn scale(v: f32, Range { start, end }: &Range<f32>) -> f32 {
            (v.clamp(*start, *end) - start) / (end - start)
        }

        Relative(Sample {
            amplitude: scale(self.amplitude, &range.amplitude),
            frequency: scale(self.frequency, &range.frequency),
        })
    }
}

impl IntoRelative for StereoFrame {
    type Range = StereoValue<AudioPatternRange>;

    fn into_relative(self, range: &StereoValue<AudioPatternRange>) -> Relative<Self> {
        Relative(StereoFrame {
            a: self.a.into_relative(&range.a).0,
            b: self.b.into_relative(&range.b).0,
        })
    }
}

impl From<f32> for Relative<f32> {
    fn from(value: f32) -> Self {
        assert!(
            (0.0..=1.0).contains(&value),
            "value outside of range 0.0..1.0: {value}"
        );
        Self(value)
    }
}
impl From<Relative<f32>> for f32 {
    #[inline]
    fn from(Relative(value): Relative<f32>) -> Self {
        value
    }
}

/// An e-stim pattern that has a single channel.

#[derive(Clone, Debug, Default)]
pub struct MonoPattern {
    /// The samples (pulses) in chronological order.
    pub samples: Vec<Sample>,
}

impl Extend<Sample> for MonoPattern {
    fn extend<I: IntoIterator<Item = Sample>>(&mut self, iter: I) {
        self.samples.extend(iter);
    }
}

impl FromIterator<Sample> for MonoPattern {
    fn from_iter<I: IntoIterator<Item = Sample>>(iter: I) -> Self {
        Self {
            samples: FromIterator::from_iter(iter),
        }
    }
}

#[cfg(feature = "stream")]
impl MonoPattern {
    /// Create a static pattern from a [`PatternStream`].
    pub async fn from_stream<E>(stream: impl PatternStream<Mono, Error = E>) -> Result<Self, E> {
        use std::pin::pin;

        use futures::TryStreamExt;

        let mut pattern = MonoPattern {
            samples: stream
                .size_hint()
                .1
                .map(Vec::with_capacity)
                .unwrap_or_default(),
        };

        let mut stream = pin!(stream);

        while let Some(sample) = stream.try_next().await? {
            pattern.samples.push(sample)
        }

        Ok(pattern)
    }

    /// Converts this pattern into a [`PatternStream`].
    pub fn into_stream(self) -> impl PatternStream<Mono, Error = std::convert::Infallible> {
        use futures::StreamExt;

        futures::stream::iter(self.samples).map(Ok)
    }
}

impl MonoPattern {
    /// Converts this pattern to a [`StereoPattern`] by cloning the channel.
    pub fn to_stereo(&self) -> StereoPattern {
        StereoPattern {
            frames: self
                .samples
                .iter()
                .map(|s| StereoFrame { a: *s, b: *s })
                .collect(),
        }
    }
}

impl MonoPattern {
    fn samples_view(&self) -> ArrayView2<'_, f32> {
        aview2(bytemuck::cast_slice::<_, [f32; 2]>(&self.samples))
    }
    fn samples_view_mut(&mut self) -> ArrayViewMut2<'_, f32> {
        aview_mut2(bytemuck::cast_slice_mut::<_, [f32; 2]>(&mut self.samples))
    }
}

impl MonoPattern {
    /// Analyses the pattern to get the minimum/maximum frequency/amplitude.
    pub fn range(&self) -> AudioPatternRange {
        let mut samples = self.samples_view().to_owned();
        let min = samples
            .quantile_axis_skipnan_mut(Axis(0), n64(0.05), &ndarray_stats::interpolate::Nearest)
            .unwrap();
        let (min_amplitude, min_frequency) = (min[0], min[1]);
        let max = samples
            .quantile_axis_skipnan_mut(Axis(0), n64(0.95), &ndarray_stats::interpolate::Nearest)
            .unwrap();
        let (max_amplitude, max_frequency) = (max[0], max[1]);

        AudioPatternRange {
            amplitude: min_amplitude..max_amplitude,
            frequency: min_frequency..max_frequency,
        }
    }
    /// Scales the frequency/amplitude based on the minimum/maximum frequency/amplitude.
    pub fn into_relative(self) -> Relative<MonoPattern> {
        let range = self.range();
        self.into_relative_in(&range)
    }
    /// Scales the frequency/amplitude relative to the given range.
    pub fn into_relative_in(mut self, range: &AudioPatternRange) -> Relative<MonoPattern> {
        let mut view = self.samples_view_mut();

        for (slice, range) in [(s![.., 0], &range.amplitude), (s![.., 1], &range.frequency)] {
            view.slice_mut(slice)
                .mapv_inplace(|v| v.into_relative(range).0);
        }

        Relative(MonoPattern {
            samples: self.samples,
        })
    }
}

/// An e-stim pattern that has a two channels.
#[derive(Clone, Debug, Default)]
pub struct StereoPattern {
    /// The frames (pulses per channel) in chronological order.
    pub frames: Vec<StereoFrame>,
}

impl Extend<StereoFrame> for StereoPattern {
    fn extend<I: IntoIterator<Item = StereoFrame>>(&mut self, iter: I) {
        self.frames.extend(iter);
    }
}

impl FromIterator<StereoFrame> for StereoPattern {
    fn from_iter<I: IntoIterator<Item = StereoFrame>>(iter: I) -> Self {
        Self {
            frames: FromIterator::from_iter(iter),
        }
    }
}

#[cfg(feature = "stream")]
impl StereoPattern {
    /// Create a static pattern from a [`PatternStream`].
    pub async fn from_stream<E>(stream: impl PatternStream<Stereo, Error = E>) -> Result<Self, E> {
        use std::pin::pin;

        use futures::TryStreamExt;

        let mut pattern = StereoPattern {
            frames: stream
                .size_hint()
                .1
                .map(Vec::with_capacity)
                .unwrap_or_default(),
        };

        let mut stream = pin!(stream);

        while let Some(frame) = stream.try_next().await? {
            pattern.frames.push(frame)
        }

        Ok(pattern)
    }

    /// Converts this pattern into a [`PatternStream`].
    pub fn into_stream(self) -> impl PatternStream<Stereo, Error = std::convert::Infallible> {
        use futures::StreamExt;

        futures::stream::iter(self.frames).map(Ok)
    }
}

impl StereoPattern {
    fn frames_view(&self) -> ArrayView3<'_, f32> {
        ArrayView3::from_shape(
            (self.frames.len(), 2, 2),
            bytemuck::cast_slice(&self.frames),
        )
        .expect("incorrect shape")
    }
    fn frames_view_mut(&mut self) -> ArrayViewMut3<'_, f32> {
        ArrayViewMut3::from_shape(
            (self.frames.len(), 2, 2),
            bytemuck::cast_slice_mut(&mut self.frames),
        )
        .expect("incorrect shape")
    }
}

impl StereoPattern {
    /// Analyses the pattern to get the minimum/maximum frequency/amplitude per channel.
    pub fn range(&self) -> StereoValue<AudioPatternRange> {
        let mut samples = self.frames_view().to_owned();
        let min = samples
            .quantile_axis_skipnan_mut(Axis(0), n64(0.05), &ndarray_stats::interpolate::Nearest)
            .unwrap();
        let (a_min_amplitude, a_min_frequency, b_min_amplitude, b_min_frequency) =
            (min[(0, 0)], min[(0, 1)], min[(1, 0)], min[(1, 1)]);
        let max = samples
            .quantile_axis_skipnan_mut(Axis(0), n64(0.95), &ndarray_stats::interpolate::Nearest)
            .unwrap();
        let (a_max_amplitude, a_max_frequency, b_max_amplitude, b_max_frequency) =
            (max[(0, 0)], max[(0, 1)], max[(1, 0)], max[(1, 1)]);

        StereoValue {
            a: AudioPatternRange {
                amplitude: a_min_amplitude..a_max_amplitude,
                frequency: a_min_frequency..a_max_frequency,
            },
            b: AudioPatternRange {
                amplitude: b_min_amplitude..b_max_amplitude,
                frequency: b_min_frequency..b_max_frequency,
            },
        }
    }
    /// Scales the frequency/amplitude per channel based on the minimum/maximum frequency/amplitude.
    pub fn into_relative(self) -> Relative<StereoPattern> {
        let range = self.range();
        self.into_relative_in(&range)
    }
    /// Scales the frequency/amplitude per channel relative to the given range.
    pub fn into_relative_in(
        mut self,
        StereoValue { a, b }: &StereoValue<AudioPatternRange>,
    ) -> Relative<StereoPattern> {
        let mut view = self.frames_view_mut();

        for (slice, range) in [
            (s![.., 0, 0], &a.amplitude),
            (s![.., 0, 1], &a.frequency),
            (s![.., 1, 0], &b.amplitude),
            (s![.., 1, 1], &b.frequency),
        ] {
            view.slice_mut(slice)
                .mapv_inplace(|v| v.into_relative(range).0);
        }

        Relative(StereoPattern {
            frames: self.frames,
        })
    }
}

impl Relative<StereoPattern> {
    const HWL_HEADER: &'static str = "YEAHBOI!";

    /// Reads a `.hwl` file from disk.
    pub fn from_hwl_file(path: impl AsRef<Path>) -> std::io::Result<Self> {
        Self::read_hwl(std::fs::File::open(path)?)
    }
    /// Reads a `.hwl` file.
    pub fn read_hwl(mut r: impl std::io::Read) -> std::io::Result<Self> {
        let mut header = [0u8; 8];
        r.read_exact(&mut header)?;
        if header != Self::HWL_HEADER.as_bytes() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("file doesnt start with {}", Self::HWL_HEADER),
            ));
        }

        let mut frames: Vec<StereoFrame> = Vec::new();

        let mut frame = [0u8; 16];
        loop {
            match r.read_exact(&mut frame) {
                Ok(()) => {
                    frames.push(StereoFrame {
                        a: Sample {
                            amplitude: f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]),
                            frequency: f32::from_le_bytes([
                                frame[8], frame[9], frame[10], frame[11],
                            ]),
                        },
                        b: Sample {
                            amplitude: f32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]),
                            frequency: f32::from_le_bytes([
                                frame[12], frame[13], frame[14], frame[15],
                            ]),
                        },
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(Relative(StereoPattern { frames }));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Writes a `.hwl` file to disk.
    pub fn write_hwl_file(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        self.write_hwl(std::fs::File::create(path)?)
    }
    /// Writes a `.hwl` file.
    pub fn write_hwl(&self, mut w: impl std::io::Write) -> std::io::Result<()> {
        w.write_all(Self::HWL_HEADER.as_bytes())?;

        for frame in &self.as_ref().frames {
            w.write_all(
                &[
                    frame.a.amplitude.to_le_bytes(),
                    frame.b.amplitude.to_le_bytes(),
                    frame.a.frequency.to_le_bytes(),
                    frame.b.frequency.to_le_bytes(),
                ]
                .concat(),
            )?;
        }

        Ok(())
    }
}

/// Holds a minimum/maximum frequency/amplitude range.
#[derive(Clone, Debug, Default)]
pub struct AudioPatternRange {
    /// The minimum/maximum amplitude.
    pub amplitude: Range<f32>,
    /// The minimum/maximum frequency.
    pub frequency: Range<f32>,
}

/// A frame with two channels. One frame corresponds to one pulse on two channels simultanously.
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct StereoFrame {
    /// The left/A channel.
    pub a: Sample,
    /// The right/B channel.
    pub b: Sample,
}

impl From<StereoValue<Sample>> for StereoFrame {
    fn from(StereoValue { a, b }: StereoValue<Sample>) -> Self {
        StereoFrame { a, b }
    }
}

impl From<StereoFrame> for StereoValue<Sample> {
    fn from(StereoFrame { a, b }: StereoFrame) -> Self {
        StereoValue { a, b }
    }
}

/// A sample with two channels. One sample corresponds to one pulse on one channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct Sample {
    /// The amplitude as a dimensionless number or in the range 0.0..=1.0 if wrapped inside [`Relative<T>`].
    pub amplitude: f32,
    /// The frequency in Hz or in the range 0.0..=1.0 if wrapped inside [`Relative<T>`].
    pub frequency: f32,
}
