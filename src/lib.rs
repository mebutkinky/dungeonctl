//! This library provides support for controlling smart electronic toys created by DG-LAB.
//!
//! The [`device`](crate::device) subcrate implements the Bluetooth LE protocol to control the specific devices.
//!
//! The [`pattern`](crate::pattern) subcrate implements representations for e-stim patterns that can be send to the devices.
//!
//! The [`audio`](crate::audio) subcrate allows generating e-stim patterns from audio inputs.
//!
//! The [`graph`](crate::graph) subcrate provides visualizations of e-stim patterns.
//!
//! The [`core`](crate::core) and [`utils`](crate::utils) subcrates provide building blocks that are used by the other subcrates;

#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![warn(missing_docs)]
// cargo-rdme can't resolve links without explicit target
#![allow(rustdoc::redundant_explicit_links)]

pub use dungeonctl_core as core;
pub use dungeonctl_utils as utils;

#[cfg(feature = "audio")]
pub use dungeonctl_audio as audio;

#[cfg(feature = "device")]
pub use dungeonctl_device as device;

#[cfg(feature = "graph")]
pub use dungeonctl_graph as graph;

#[cfg(feature = "pattern")]
pub use dungeonctl_pattern as pattern;
