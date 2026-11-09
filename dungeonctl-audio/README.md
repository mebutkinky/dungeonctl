# dungeonctl-audio

See [dungeonctl](../README.md) for more information.

<!-- cargo-rdme start -->

This library allows generating e-stim patterns from audio inputs.

It provides the following backends:

- `symphonia` for reading audio files
  - provides [`audio_file`](https://docs.rs/dungeonctl-audio/latest/dungeonctl_audio/symphonia/fn.audio_file.html) for reading an audio file from disk
- `pipewire` for integration with the pipewire audio system (only on Linux)
  - provides [`pipewire_sink`](https://docs.rs/dungeonctl-audio/latest/dungeonctl_audio/pipewire/fn.pipewire_sink.html) for creating a virtual audio device

Use [`AudioStream::audio_pattern()`](https://docs.rs/dungeonctl-audio/latest/dungeonctl_audio/audio_stream/struct.AudioStream.html#method.audio_pattern) to convert an audio stream into a [`PatternStream`](https://docs.rs/dungeonctl_pattern/latest/dungeonctl_pattern/stream/trait.PatternStream.html).

<!-- cargo-rdme end -->
