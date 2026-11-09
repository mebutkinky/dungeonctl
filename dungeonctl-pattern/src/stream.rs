//! Provides [`Stream`] implementations, specifically the helper trait [`PatternStream`].

use std::task::Poll;

use arrayvec::ArrayVec;
use async_stream::try_stream;
use dungeonctl_core::StereoValue;
use futures::{Stream, StreamExt, TryStream, TryStreamExt};

use crate::{
    AudioPatternRange, IntoRelative, Mono, Relative, Sample, Stereo, StereoFrame,
    channels::ChannelLayout,
};

/// A [`Stream`] with an item type of `Result<Sample, E>` (as [`PatternStream<Mono>`]) or `Result<StereoFrame, E>` (as [`PatternStream<Stereo>`]).
#[cfg_attr(docsrs, doc(notable_trait))]
pub trait PatternStream<L: ChannelLayout>:
    Stream<Item = Result<L::Frame, Self::Error>> + TryStream<Ok = L::Frame>
{
}

impl<S> PatternStream<Mono> for S where
    Self: Stream<Item = Result<Sample, Self::Error>> + TryStream<Ok = Sample>
{
}
impl<S> PatternStream<Stereo> for S where
    Self: Stream<Item = Result<StereoFrame, Self::Error>> + TryStream<Ok = StereoFrame>
{
}

/// Provides additional methods for [`PatternStream`].
#[cfg_attr(docsrs, doc(notable_trait))]
pub trait PatternStreamExt<L: ChannelLayout>: PatternStream<L> {
    /// Converts this pattern stream into a stream of chunks with a known size.
    ///
    /// Remaining frames are discarded, similarly to [`StreamExt::chunks()`]
    fn const_chunks<const LEN: usize>(
        mut self,
    ) -> impl TryStream<
        Ok = [L::Frame; LEN],
        Error = Self::Error,
        Item = Result<[L::Frame; LEN], Self::Error>,
    >
    where
        Self: Sized + Unpin,
    {
        try_stream! {
            loop {
                let mut chunk: ArrayVec<L::Frame, LEN> = ArrayVec::new_const();

                for _ in 0..LEN {
                    match self.try_next().await? {
                        Some(frame) => chunk.push(frame),
                        None => return,
                    }
                }

                match chunk.into_inner() {
                    Ok(chunk) => yield chunk,
                    Err(_) => unreachable!()
                }
            }
        }
    }
}

impl<S> PatternStreamExt<Mono> for S where S: PatternStream<Mono> {}
impl<S> PatternStreamExt<Stereo> for S where S: PatternStream<Stereo> {}

/// Provides additional methods for [`PatternStream<Mono>`].
#[cfg_attr(docsrs, doc(notable_trait))]
pub trait MonoPatternStreamExt<E>
where
    Self: PatternStream<Mono, Error = E>,
{
    /// Converts this stream into a [`PatternStream<Stereo>`] by copying each sample.
    fn into_stereo(self) -> impl PatternStream<Stereo, Error = E>
    where
        Self: Sized,
    {
        self.map_ok::<StereoFrame, _>(|sample| StereoFrame {
            a: sample,
            b: sample,
        })
    }
    /// Scales the frequency/amplitude relative to the given range.
    fn into_relative(
        self,
        range: AudioPatternRange,
    ) -> Relative<impl PatternStream<Mono, Error = E>>
    where
        Self: Sized,
    {
        Relative(self.map_ok(move |frame: Sample| frame.into_relative(&range).0))
    }
}
impl<S, E> MonoPatternStreamExt<E> for S where Self: PatternStream<Mono, Error = E> + Sized {}

/// Provides additional methods for [`PatternStream<Stereo>`].
#[cfg_attr(docsrs, doc(notable_trait))]
pub trait StereoPatternStreamExt<E>
where
    Self: PatternStream<Stereo, Error = E>,
{
    /// Scales the frequency/amplitude relative to the given range.
    fn into_relative(
        self,
        range: StereoValue<AudioPatternRange>,
    ) -> Relative<impl PatternStream<Stereo, Error = E>>
    where
        Self: Sized,
    {
        Relative(self.map_ok(move |frame: StereoFrame| frame.into_relative(&range).0))
    }
}
impl<S, E> StereoPatternStreamExt<E> for S where Self: PatternStream<Stereo, Error = E> + Sized {}

impl<S: Stream + Unpin> Stream for Relative<S> {
    type Item = S::Item;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.0.poll_next_unpin(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}
