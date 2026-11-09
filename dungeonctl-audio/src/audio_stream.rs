use std::{
    marker::PhantomData,
    pin::{Pin, pin},
    sync::Arc,
    task::Poll,
};

use futures::Stream;
use pin_project_lite::pin_project;
use thingbuf::{ThingBuf, recycling::WithCapacity};

use crate::{
    AnalyzeAudio,
    channel_layout::{AudioBuffer, ChannelLayout},
};

pin_project! {
    /// A stream of audio data which can be converted into a pattern stream.
    ///
    /// This doesn't implement [`Stream`] in order to be able to reuse audio buffers and improving performance.
    pub struct AudioStream<L, E>
    where
        L: ChannelLayout,
    {
        queue: Arc<ThingBuf<Vec<f32>, WithCapacity>>,
        sample_rate: usize,
        n_frames: Option<usize>,
        #[pin]
        result_stream: AudioResultStream<L, E>,

        layout: PhantomData<L>,
    }
}

impl<L, E> AudioStream<L, E>
where
    L: ChannelLayout,
{
    pub(crate) fn new(
        queue: Arc<ThingBuf<Vec<f32>, WithCapacity>>,
        sample_rate: usize,
        n_frames: Option<usize>,
        result_stream: AudioResultStream<L, E>,
    ) -> Self {
        Self {
            sample_rate,
            n_frames,
            queue,
            result_stream,
            layout: PhantomData,
        }
    }
}

impl<L, E> AudioStream<L, E>
where
    L: ChannelLayout,
{
    pub(crate) fn sample_rate(&self) -> usize {
        self.sample_rate
    }
    pub(crate) fn n_frames(&self) -> Option<usize> {
        self.n_frames
    }
    /// Convert this audio stream into a pattern stream using the YIN algorithm for frequency detection.
    pub fn audio_pattern(self) -> AnalyzeAudio<L, E> {
        AnalyzeAudio::new(self)
    }
}

impl<L, E> AudioStream<L, E>
where
    L: ChannelLayout,
{
    pub(crate) fn poll_next_ref<'a>(
        self: Pin<&'a mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<AudioChunkRef<'a, L>, E>>> {
        let this = self.project();

        match this.result_stream.poll_next(cx) {
            Poll::Ready(Some(Ok(()))) => Poll::Ready(Some(Ok(AudioChunkRef {
                ref_: this.queue.pop_ref().expect("audio queue is empty"),
                layout: PhantomData,
            }))),
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Debug)]
pub struct AudioChunkRef<'a, L: ChannelLayout> {
    ref_: thingbuf::Ref<'a, Vec<f32>>,
    layout: PhantomData<L>,
}

impl<L: ChannelLayout> AudioChunkRef<'_, L> {
    pub fn get(&self) -> L::AudioBuffer<'_> {
        L::AudioBuffer::from_planes(&self.ref_)
    }
}

pub(crate) type AudioQueue = Arc<ThingBuf<Vec<f32>, WithCapacity>>;

pub(crate) fn audio_queue(capacity: usize) -> AudioQueue {
    Arc::new(thingbuf::ThingBuf::with_recycle(
        capacity,
        WithCapacity::new(),
    ))
}

pin_project! {
    pub(crate) struct AudioResultStream<L: ChannelLayout, E> {
        pub audio_task: Option<blocking::Task<Result<(), E>>>,
        pub cancel_tx: crate::utils::CancellationSender,
        #[pin]
        pub result_rx: async_channel::Receiver<Result<(), E>>,

        pub layout: PhantomData<L>
    }
}

impl<L: ChannelLayout, E> Stream for AudioResultStream<L, E> {
    type Item = Result<(), E>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        match &mut this.audio_task {
            None => Poll::Ready(None),
            Some(task_handle) => match pin!(task_handle).poll(cx) {
                Poll::Ready(res) => {
                    *this.audio_task = None;

                    match res {
                        Ok(()) => Poll::Ready(None),
                        Err(e) => Poll::Ready(Some(Err(e))),
                    }
                }
                Poll::Pending => this.result_rx.poll_next(cx),
            },
        }
    }
}
