//! Reads and decodes an audio file

use std::{marker::PhantomData, path::PathBuf};

use dungeonctl_pattern::{Mono, Stereo};
use eyre::{OptionExt, WrapErr};
use symphonia::core::{
    audio::{Audio, AudioBuffer},
    codecs::{CodecParameters, audio::AudioDecoder as AudioDecoderT},
    formats::{FormatReader, TrackType, probe::Hint},
    io::MediaSourceStream,
};
use tracing::debug;

use crate::audio_stream::{AudioResultStream, AudioStream, audio_queue};

pub use either::Either;
pub use symphonia::core::io::MediaSource;

/// Reads audio data from an audio file.
pub async fn audio_file(
    src: impl AudioSource,
) -> eyre::Result<Either<AudioStream<Mono, eyre::Report>, AudioStream<Stereo, eyre::Report>>> {
    let (cancel_tx, cancel_rx) = crate::utils::cancel();
    let (params_tx, params_rx) = futures::channel::oneshot::channel::<AudioParams>();
    let queue = audio_queue(3);
    let (result_tx, result_rx) = async_channel::bounded::<eyre::Result<()>>(1);

    let decoding_task = blocking::unblock({
        let queue = queue.clone();

        move || -> eyre::Result<()> {
            let mut decoder = AudioDecoder::load_audio_file(src)?;

            params_tx.send(decoder.params).unwrap();

            match decoder.params.channels {
                Either::Left(Mono) => loop {
                    let Some(res) = decoder.next() else {
                        return Ok(());
                    };

                    if cancel_rx.is_canceled() {
                        return Ok(());
                    } else {
                        match res {
                            Ok(buf) => {
                                queue
                                    .push_with(|shared_buf| {
                                        shared_buf.extend_from_slice(
                                            buf.plane(0)
                                                .expect("audio buffer must contain a channel"),
                                        )
                                    })
                                    .expect("audio queue full");
                                result_tx.send_blocking(Ok(()))
                            }
                            Err(e) => result_tx.send_blocking(Err(e)),
                        }?
                    }
                },
                Either::Right(Stereo) => loop {
                    let Some(res) = decoder.next() else {
                        return Ok(());
                    };

                    if cancel_rx.is_canceled() {
                        return Ok(());
                    } else {
                        match res {
                            Ok(buf) => {
                                queue
                                    .push_with(|shared_buf| {
                                        shared_buf.reserve(2 * buf.frames());
                                        let (left, right) = buf
                                            .plane_pair(0, 1)
                                            .expect("audio buffer must contain two channels");
                                        shared_buf.extend_from_slice(left);
                                        shared_buf.extend_from_slice(right);
                                    })
                                    .expect("audio queue full");
                                result_tx.send_blocking(Ok(()))
                            }
                            Err(e) => result_tx.send_blocking(Err(e)),
                        }?
                    }
                },
            }
        }
    });

    let AudioParams {
        channels,
        sample_rate,
        num_frames: n_frames,
    } = match params_rx.await {
        Ok(params) => params,
        Err(e) => {
            decoding_task.await?;
            return Err(e.into());
        }
    };

    match channels {
        Either::Left(Mono) => Ok(Either::Left(AudioStream::new(
            queue,
            sample_rate,
            n_frames,
            AudioResultStream {
                audio_task: Some(decoding_task),
                cancel_tx,
                result_rx,
                layout: PhantomData::<Mono>,
            },
        ))),
        Either::Right(Stereo) => Ok(Either::Right(AudioStream::new(
            queue,
            sample_rate,
            n_frames,
            AudioResultStream {
                audio_task: Some(decoding_task),
                cancel_tx,
                result_rx,
                layout: PhantomData::<Stereo>,
            },
        ))),
    }
}

#[derive(Clone, Copy, Debug)]
struct AudioParams {
    channels: Either<Mono, Stereo>,
    sample_rate: usize,
    num_frames: Option<usize>,
}

struct AudioDecoder {
    params: AudioParams,
    track_id: u32,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoderT>,
}

impl AudioDecoder {
    fn load_audio_file(src: impl AudioSource) -> eyre::Result<AudioDecoder> {
        let mut hint = Hint::new();
        if let Some(ext) = src.ext() {
            hint.with_extension(ext);
            debug!(?hint);
        }

        let mss = MediaSourceStream::new(src.into_media_source()?, Default::default());

        let format = symphonia::default::get_probe().probe(
            &hint,
            mss,
            Default::default(),
            Default::default(),
        )?;

        let track = format
            .first_track(TrackType::Audio)
            .ok_or_eyre("no supported audio tracks")?;
        let Some(CodecParameters::Audio(ref codec_params)) = track.codec_params else {
            eyre::bail!("audio track must have audio codec parameters")
        };

        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(codec_params, &Default::default())
            .wrap_err("unsupported codec")?;

        let track_id = track.id;

        let channels: Either<Mono, Stereo> = match codec_params
            .channels
            .as_ref()
            .ok_or_eyre("unknown channel count")?
            .count()
        {
            1 => Either::Left(Mono),
            2 => Either::Right(Stereo),
            n => eyre::bail!(
                "audio file has {n} channels, but only mono or stereo audio is supported"
            ),
        };

        let n_frames = track.num_frames.map(|n| n as usize);

        let sample_rate = codec_params.sample_rate.ok_or_eyre("unknown sample rate")? as usize;

        Ok(Self {
            params: AudioParams {
                channels,
                sample_rate,
                num_frames: n_frames,
            },
            track_id,
            format,
            decoder,
        })
    }
}

impl Iterator for AudioDecoder {
    type Item = eyre::Result<AudioBuffer<f32>>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Get the next packet from the media format.
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return None,
                Err(e) => return Some(Err(e.into())),
            };

            // Consume any new metadata that has been read since the last packet.
            while !self.format.metadata().is_latest() {
                // Pop the old head of the metadata queue.
                self.format.metadata().pop();

                // Consume the new metadata at the head of the metadata queue.
            }

            // If the packet does not belong to the selected track, skip over it.
            if packet.track_id != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(buffer_ref) => {
                    let mut buffer =
                        AudioBuffer::<f32>::new(buffer_ref.spec().clone(), buffer_ref.frames());
                    buffer.render_uninit(None);
                    buffer_ref.copy_to(&mut buffer);
                    return Some(Ok(buffer));
                }
                Err(e) => {
                    return Some(Err(
                        eyre::Report::from(e).wrap_err("failed to decode packet")
                    ));
                }
            };
        }
    }
}

/// An audio file that can be read by symphonia.
pub trait AudioSource: Send + Sync + 'static {
    /// Create a [`symphonia::core::io::MediaSource`].
    fn into_media_source(self) -> std::io::Result<Box<dyn MediaSource>>;
    /// An optional file extension that can be used as a hint for the audio decoder.
    fn ext(&self) -> Option<&str> {
        None
    }
}

impl AudioSource for PathBuf {
    fn into_media_source(self) -> std::io::Result<Box<dyn MediaSource>> {
        debug!("reading audio file at '{}'", self.display());

        let file = std::fs::File::open(self)?;
        Ok(Box::new(file))
    }
    fn ext(&self) -> Option<&str> {
        self.extension().and_then(|s| s.to_str())
    }
}

impl AudioSource for std::fs::File {
    fn into_media_source(self) -> std::io::Result<Box<dyn MediaSource>> {
        Ok(Box::new(self))
    }
}

impl<T: AsRef<[u8]> + Send + Sync + 'static> AudioSource for std::io::Cursor<T> {
    fn into_media_source(self) -> std::io::Result<Box<dyn MediaSource>> {
        Ok(Box::new(self))
    }
}
