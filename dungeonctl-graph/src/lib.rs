//! Provides visualizations of e-stim patterns.

#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![warn(missing_docs)]

use dungeonctl_pattern::{MonoPattern, Relative, StereoPattern};
use palette::{FromColor, Hsv, Srgb};
use svg::{
    Document, Node,
    node::element::{Line, Rectangle, Text},
};

/// Converts an e-stim pattern to svg.
pub trait PatternToSvg {
    /// Converts an e-stim pattern to svg.
    fn to_svg(&self, step: usize) -> svg::Document;
}

impl PatternToSvg for MonoPattern {
    fn to_svg(&self, step_size: usize) -> svg::Document {
        let range = self.range();

        self.clone().into_relative_in(&range).to_svg(step_size).add(
            Text::new(format!(
                "{} Hz - {} Hz",
                range.frequency.start, range.frequency.end
            ))
            .set("x", 10)
            .set("y", 90),
        )
    }
}

impl PatternToSvg for Relative<MonoPattern> {
    fn to_svg(&self, step_size: usize) -> svg::Document {
        let samples = &self.0.samples;

        let mut document = Document::new()
            .set("width", samples.len())
            .set("height", 100)
            .set("viewBox", (0, 0, samples.len(), 100))
            .set("preserveAspectRatio", "xMinYMid meet");

        for (i, sample) in samples.iter().step_by(step_size).enumerate() {
            let color: Srgb<u8> =
                Srgb::from_color(Hsv::new_srgb(120.0 * (1.0 - sample.frequency), 1.0, 1.0))
                    .into_format();
            let height = (100.0 * sample.amplitude).round() as u8;
            document.append(
                Rectangle::new()
                    .set("x", i)
                    .set("y", 100 - height)
                    .set("width", 1)
                    .set("height", height)
                    .set("stroke", "none")
                    .set(
                        "fill",
                        format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue),
                    ),
            );
        }

        document
    }
}

impl PatternToSvg for StereoPattern {
    fn to_svg(&self, step_size: usize) -> svg::Document {
        let range = self.range();

        self.clone()
            .into_relative_in(&range)
            .to_svg(step_size)
            .add(
                Text::new(format!(
                    "A: {} Hz - {} Hz",
                    range.a.frequency.start, range.a.frequency.end
                ))
                .set("x", 10)
                .set("y", 90),
            )
            .add(
                Text::new(format!(
                    "B: {} Hz - {} Hz",
                    range.b.frequency.start, range.b.frequency.end
                ))
                .set("x", 10)
                .set("y", 200),
            )
    }
}

impl PatternToSvg for Relative<StereoPattern> {
    fn to_svg(&self, step_size: usize) -> svg::Document {
        let frames = &self.0.frames;

        let mut document = Document::new()
            .set("width", frames.len() / step_size)
            .set("height", 220)
            .set("viewBox", (0, 0, frames.len() / step_size, 220))
            .set("preserveAspectRatio", "xMinYMid meet");

        document.append(
            Line::new()
                .set("x1", 0)
                .set("y1", 105)
                .set("x2", frames.len() / step_size)
                .set("y2", 105)
                .set("stroke", "#000"),
        );

        for (i, frame) in frames.iter().step_by(step_size).enumerate() {
            let sample = frame.a;
            let color: Srgb<u8> =
                Srgb::from_color(Hsv::new_srgb(120.0 * (1.0 - sample.frequency), 1.0, 1.0))
                    .into_format();
            let height = (100.0 * sample.amplitude).round() as u8;
            document.append(
                Rectangle::new()
                    .set("x", i)
                    .set("y", 100 - height)
                    .set("width", 1)
                    .set("height", height)
                    .set("stroke", "none")
                    .set(
                        "fill",
                        format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue),
                    ),
            );

            let sample = frame.b;
            let color: Srgb<u8> =
                Srgb::from_color(Hsv::new_srgb(120.0 * (1.0 - sample.frequency), 1.0, 1.0))
                    .into_format();
            let height = (100.0 * sample.amplitude).round() as u8;

            document.append(
                Rectangle::new()
                    .set("x", i)
                    .set("y", 110)
                    .set("width", 1)
                    .set("height", height)
                    .set("stroke", "none")
                    .set(
                        "fill",
                        format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue),
                    ),
            );
        }

        for x in (0..frames.len()).step_by(5 * 60 * 40) {
            document.append(
                Line::new()
                    .set("x1", x / step_size)
                    .set("y1", 0)
                    .set("x2", x / step_size)
                    .set("y2", 220)
                    .set("stroke", "#000"),
            );
        }

        document
    }
}
