//! This library allows generating e-stim patterns from audio inputs.
//!
//! It provides the following backends:
//!
//! - `symphonia` for reading audio files
//!   - provides [`audio_file`](crate::symphonia::audio_file) for reading an audio file from disk
//! - `pipewire` for integration with the pipewire audio system (only on Linux)
//!   - provides [`pipewire_sink`](crate::pipewire::pipewire_sink) for creating a virtual audio device
//!
//! Use [`AudioStream::audio_pattern()`] to convert an audio stream into a [`PatternStream`](dungeonctl_pattern::stream::PatternStream).

#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![warn(missing_docs)]

mod analyze;
mod audio_stream;
mod channel_layout;
#[cfg(all(feature = "pipewire", any(target_os = "linux", target_os = "freebsd")))]
pub mod pipewire;
#[cfg(feature = "symphonia")]
pub mod symphonia;
mod utils;
mod yin;

pub use self::{analyze::AnalyzeAudio, audio_stream::AudioStream, channel_layout::ChannelLayout};
