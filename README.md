# dungeonctl

[![Build Status](https://github.com/mebutkinky/dungeonctl/workflows/CI/badge.svg)](https://github.com/mebutkinky/dungeonctl/actions?query=workflow%3ACI)
[![Crates.io](https://img.shields.io/crates/v/dungeonctl)](https://lib.rs/crates/dungeonctl)
[![Documentation](https://img.shields.io/docsrs/dungeonctl)](https://docs.rs/dungeonctl)
[![License: MIT](https://img.shields.io/crates/l/dungeonctl)](LICENSE)

<!-- cargo-rdme start -->

This library provides support for controlling smart electronic toys created by DG-LAB.

The [`device`](https://docs.rs/dungeonctl_device/latest/dungeonctl_device/) subcrate implements the Bluetooth LE protocol to control the specific devices.

The [`pattern`](https://docs.rs/dungeonctl_pattern/latest/dungeonctl_pattern/) subcrate implements representations for e-stim patterns that can be send to the devices.

The [`audio`](https://docs.rs/dungeonctl_audio/latest/dungeonctl_audio/) subcrate allows generating e-stim patterns from audio inputs.

The [`graph`](https://docs.rs/dungeonctl_graph/latest/dungeonctl_graph/) subcrate provides visualizations of e-stim patterns.

The [`core`](https://docs.rs/dungeonctl_core/latest/dungeonctl_core/) and [`utils`](https://docs.rs/dungeonctl_utils/latest/dungeonctl_utils/) subcrates provide building blocks that are used by the other subcrates;

<!-- cargo-rdme end -->

## License

This project is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
