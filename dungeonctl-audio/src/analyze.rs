use std::{pin::Pin, task::Poll};

use dungeonctl_pattern::{PULSE_INTERVAL, Sample};
use futures::Stream;
use ndarray::{Array, aview1};
use pin_project_lite::pin_project;

use crate::{
    audio_stream::AudioStream,
    channel_layout::{AudioBuffer, ChannelArray, ChannelLayout, PatternFrame},
    yin::Yin,
};

pin_project! {
    /// A [`PatternStream`](dungeonctl_pattern::stream::PatternStream) that consumes audio data and does frequency detection using the [YIN] algorithm to create a pattern.
    ///
    /// [YIN]: http://audition.ens.fr/adc/pdf/2002_JASA_YIN.pdf
    #[project = AnalyzeAudioProj]
    pub struct AnalyzeAudio<L, E>
    where
        L: ChannelLayout,
    {
        #[pin]
        audio: AudioStream<L, E>,
        sample_rate: usize,
        chunk_len: usize,
        window_len: usize,
        buf: L::Array<Vec<f32>>,
        yin: L::Array<Yin>,
        last_frequency: L::Array<f32>,
    }
}

const MIN_FREQUENCY: f32 = 200.0;
const MAX_FREQUENCY: f32 = 4000.0;
const HOPS_PER_CHUNK: usize = 8;

impl<L, E> AnalyzeAudio<L, E>
where
    L: ChannelLayout,
{
    pub(crate) fn new(audio: AudioStream<L, E>) -> Self {
        let sample_rate = audio.sample_rate();
        let chunk_len = sample_rate * PULSE_INTERVAL.as_millis() as usize / 1000;
        let window_len = chunk_len.next_power_of_two();

        let buf = L::Array::from_fn(|_| Vec::with_capacity(window_len));
        let yin = L::Array::from_fn(|_| Yin::new(window_len, sample_rate, 0.15));

        let last_frequency = L::Array::from_fn(|_| MIN_FREQUENCY);

        Self {
            audio,
            sample_rate,
            chunk_len,
            window_len,
            buf,
            yin,
            last_frequency,
        }
    }
}

impl<L, E> AnalyzeAudioProj<'_, L, E>
where
    L: ChannelLayout,
{
    fn next_frame(&mut self) -> Option<L::PatternFrame> {
        let extended_window_len = *self.chunk_len + *self.window_len;

        if self.buf.as_ref()[0].len() >= extended_window_len {
            let mut samples: L::Array<Sample> = L::Array::from_fn(|_| Sample::default());

            for (channel, channel_buf) in self.buf.as_mut().iter_mut().enumerate() {
                let extended_window = &channel_buf[..extended_window_len];

                let hop_len = *self.chunk_len / HOPS_PER_CHUNK;
                let mut frequencies = Array::default((HOPS_PER_CHUNK,));
                let mut amplitudes = Array::default((HOPS_PER_CHUNK,));

                for i in 0..HOPS_PER_CHUNK {
                    let offset = i * hop_len;
                    let window = &extended_window[offset..offset + *self.window_len];
                    frequencies[i] =
                        self.yin.as_mut()[channel].pitch(window, Some(200.0..20_000.0));
                    amplitudes[i] = aview1(extended_window).abs().mean().unwrap_or(0.0);
                }

                // let median_frequency = frequencies
                //     .quantile_axis_skipnan_mut(
                //         Axis(0),
                //         n64(0.5),
                //         &ndarray_stats::interpolate::Nearest,
                //     )
                //     .unwrap()
                //     .into_scalar();
                let median_frequency = frequencies.mean().unwrap();
                let frequency = if (MIN_FREQUENCY..=MAX_FREQUENCY).contains(&median_frequency) {
                    median_frequency
                } else {
                    self.last_frequency.as_mut()[channel]
                };
                let amplitude = amplitudes.mean().unwrap();

                // let mut frequency = self.yin.as_mut()[channel].pitch(window, Some(200.0..20_000.0));
                // if !(MIN_FREQUENCY..=MAX_FREQUENCY).contains(&frequency) {
                //     frequency = self.last_frequency.as_mut()[channel];
                // }
                // let amplitude = aview1(window).abs().mean().unwrap_or(0.0);

                samples.as_mut()[channel] = Sample {
                    amplitude,
                    frequency,
                };
                self.last_frequency.as_mut()[channel] = frequency;

                channel_buf.drain(..*self.chunk_len);
            }

            Some(L::PatternFrame::from_array(samples))
        } else {
            None
        }
    }
}

impl<L, E> Stream for AnalyzeAudio<L, E>
where
    L: ChannelLayout,
{
    type Item = Result<L::PatternFrame, E>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.as_mut().project();

        loop {
            if let Some(frame) = this.next_frame() {
                return Poll::Ready(Some(Ok(frame)));
            }

            match this.audio.as_mut().poll_next_ref(cx) {
                Poll::Ready(Some(Ok(buffer))) => {
                    for (channel_buf, chunk) in this
                        .buf
                        .as_mut()
                        .iter_mut()
                        .zip(buffer.get().iter_channels())
                    {
                        channel_buf.extend(chunk);
                    }
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.audio.n_frames().map(|n| n / self.chunk_len))
    }
}
