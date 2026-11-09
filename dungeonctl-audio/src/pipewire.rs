//! Provides a Pipewire sink to capture (system) audio on Linux

use std::{marker::PhantomData, time::Duration};

use pw::{
    properties::properties,
    spa::{
        param::audio::{AudioFormat, AudioInfoRaw},
        pod::{self, serialize::PodSerializer},
    },
    stream::StreamFlags,
};
use tracing::error;

use crate::{
    AudioStream,
    audio_stream::{AudioResultStream, audio_queue},
    channel_layout::ChannelLayout,
};

const SAMPLE_RATE: usize = 48_000;

/// Creates a pipewire sink as a virtual output device and reads audio data that is fed into the node.
pub fn pipewire_sink<L: ChannelLayout>() -> AudioStream<L, eyre::Report> {
    let (cancel_tx, cancel_rx) = crate::utils::cancel();

    let queue = audio_queue(2);
    let (result_tx, result_rx) = async_channel::bounded::<eyre::Result<()>>(2);

    let pw_task = blocking::unblock({
        let queue = queue.clone();

        move || {
            pw::init();

            let mainloop = pw::main_loop::MainLoopRc::new(None)?;
            let context = pw::context::ContextRc::new(&mainloop, None)?;
            let core = context.connect_rc(None)?;

            let stream = pw::stream::StreamRc::new(
                core,
                "dungeonctl",
                properties! {
                    *pw::keys::NODE_NAME => "dungeonctl",
                    *pw::keys::MEDIA_TYPE => "Audio",
                    *pw::keys::MEDIA_CATEGORY => "Playback",
                    *pw::keys::MEDIA_CLASS => "Audio/Sink",
                    *pw::keys::STREAM_CAPTURE_SINK => "true",
                },
            )?;

            let _listener: pw::stream::StreamListener<()> = stream
                .add_local_listener::<()>()
                .process({
                    let cancel_rx = cancel_rx.clone();
                    move |stream, ()| match stream.dequeue_buffer() {
                        None => println!("out of buffers"),
                        Some(mut buffer) => {
                            if cancel_rx.is_canceled() {
                                return;
                            }

                            let datas = buffer.datas_mut();
                            if datas.is_empty() {
                                return;
                            }

                            let len = datas
                                .iter()
                                .map(|data| data.chunk().size() as usize / 4)
                                .max()
                                .unwrap_or_default();

                            if len > 0 {
                                match queue.push_with(|shared_buf| {
                                    shared_buf.reserve(len * 2);

                                    for channel_data in datas {
                                        let chunk = channel_data.chunk();
                                        let start = chunk.offset() as usize;
                                        let end = start + (chunk.size() as usize);

                                        if let Some(samples) = channel_data.data() {
                                            let samples: &[f32] =
                                                bytemuck::cast_slice(&samples[start..end]);
                                            shared_buf.extend_from_slice(samples);

                                            if samples.len() > len {
                                                shared_buf.extend(std::iter::repeat_n(
                                                    0.0,
                                                    len - samples.len(),
                                                ));
                                            }
                                        } else {
                                            shared_buf.extend(std::iter::repeat_n(0.0, len));
                                        }
                                    }
                                }) {
                                    Ok(()) => {
                                        result_tx
                                            .try_send(Ok(()))
                                            .expect("should succeed if there is no buffer overrun");
                                    }
                                    Err(e) => error!("buffer overrun: {e:?}"),
                                }
                            }
                        }
                    }
                })
                .register()?;

            let mut audio_info = AudioInfoRaw::new();

            #[cfg(target_endian = "little")]
            audio_info.set_format(AudioFormat::F32P);
            #[cfg(target_endian = "big")]
            audio_info.set_format(AudioFormat::F32P);
            audio_info.set_rate(SAMPLE_RATE as u32);
            audio_info.set_channels(L::N as u32);

            let values: Vec<u8> = PodSerializer::serialize(
                std::io::Cursor::new(Vec::new()),
                &pod::Value::Object(pod::Object {
                    type_: pw::spa::sys::SPA_TYPE_OBJECT_Format,
                    id: pw::spa::sys::SPA_PARAM_EnumFormat,
                    properties: audio_info.into(),
                }),
            )
            .unwrap()
            .0
            .into_inner();

            let mut params = [pod::Pod::from_bytes(&values).unwrap()];

            stream.connect(
                pw::spa::utils::Direction::Input,
                None,
                StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
                &mut params,
            )?;

            let timer = mainloop.loop_().add_timer({
                let mainloop = mainloop.clone();
                move |_| {
                    if cancel_rx.is_canceled() {
                        mainloop.quit();
                    }
                }
            });
            timer
                .update_timer(
                    Some(Duration::from_millis(50)),
                    Some(Duration::from_millis(50)),
                )
                .into_result()?;

            mainloop.run();

            eyre::Ok(())
        }
    });

    AudioStream::new(
        queue,
        SAMPLE_RATE,
        None,
        AudioResultStream {
            audio_task: Some(pw_task),
            cancel_tx,
            result_rx,
            layout: PhantomData::<L>,
        },
    )
}
