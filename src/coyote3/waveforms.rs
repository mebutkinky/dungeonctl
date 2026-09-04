use super::{EncodedPulse, EncodedShape, EncodedShapeContext};

const BREATHING_FRAMES: &[(u8, u8)] = &[
    (10, 0),
    (10, 20),
    (10, 40),
    (10, 60),
    (10, 80),
    (10, 100),
    (10, 100),
    (10, 100),
    (10, 0),
    (10, 0),
    (10, 0),
    (10, 0),
];

const TIDAL_FRAMES: &[(u8, u8)] = &[
    (10, 0),
    (11, 16),
    (13, 33),
    (14, 50),
    (16, 66),
    (18, 83),
    (19, 100),
    (21, 92),
    (22, 84),
    (24, 76),
    (26, 68),
    (26, 0),
    (27, 16),
    (29, 33),
    (30, 50),
    (32, 66),
    (34, 83),
    (35, 100),
    (37, 92),
    (38, 84),
    (40, 76),
    (42, 68),
    (10, 0),
];

struct TableShape {
    frames: &'static [(u8, u8)],
    frame_index: usize,
}

impl TableShape {
    fn new(frames: &'static [(u8, u8)]) -> Self {
        Self {
            frames,
            frame_index: 0,
        }
    }

    fn next_pulse(&mut self, context: EncodedShapeContext) -> EncodedPulse {
        let (frequency, intensity) = self.frames[self.frame_index];
        if context.chunk_index == 3 {
            self.frame_index = (self.frame_index + 1) % self.frames.len();
        }

        EncodedPulse {
            frequency,
            intensity,
        }
    }
}

/// The twelve-frame “Breathing” waveform published with the Coyote V3 protocol.
pub struct Breathing(TableShape);

impl Breathing {
    /// Create the waveform at its first 100 ms frame.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for Breathing {
    fn default() -> Self {
        Self(TableShape::new(BREATHING_FRAMES))
    }
}

impl EncodedShape for Breathing {
    fn next_pulse(&mut self, context: EncodedShapeContext) -> EncodedPulse {
        self.0.next_pulse(context)
    }
}

/// The twenty-three-frame “Tidal” waveform published with the Coyote V3 protocol.
pub struct Tidal(TableShape);

impl Tidal {
    /// Create the waveform at its first 100 ms frame.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for Tidal {
    fn default() -> Self {
        Self(TableShape::new(TIDAL_FRAMES))
    }
}

impl EncodedShape for Tidal {
    fn next_pulse(&mut self, context: EncodedShapeContext) -> EncodedPulse {
        self.0.next_pulse(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BREATHING_EXPECTED: &[(u8, u8)] = &[
        (10, 0),
        (10, 20),
        (10, 40),
        (10, 60),
        (10, 80),
        (10, 100),
        (10, 100),
        (10, 100),
        (10, 0),
        (10, 0),
        (10, 0),
        (10, 0),
    ];

    const TIDAL_EXPECTED: &[(u8, u8)] = &[
        (10, 0),
        (11, 16),
        (13, 33),
        (14, 50),
        (16, 66),
        (18, 83),
        (19, 100),
        (21, 92),
        (22, 84),
        (24, 76),
        (26, 68),
        (26, 0),
        (27, 16),
        (29, 33),
        (30, 50),
        (32, 66),
        (34, 83),
        (35, 100),
        (37, 92),
        (38, 84),
        (40, 76),
        (42, 68),
        (10, 0),
    ];

    fn assert_waveform(shape: &mut impl EncodedShape, expected: &[(u8, u8)]) {
        for &(frequency, intensity) in expected {
            for chunk_index in 0..4 {
                let pulse = shape.next_pulse(EncodedShapeContext {
                    chunk_index,
                    ..Default::default()
                });
                assert_eq!((pulse.frequency, pulse.intensity), (frequency, intensity));
            }
        }

        let first = shape.next_pulse(EncodedShapeContext::default());
        assert_eq!(
            (first.frequency, first.intensity),
            expected[0],
            "waveform must loop to its first frame"
        );
    }

    #[test]
    fn breathing_matches_the_documented_v3_sequence() {
        assert_waveform(&mut Breathing::new(), BREATHING_EXPECTED);
    }

    #[test]
    fn tidal_matches_the_documented_v3_sequence() {
        assert_waveform(&mut Tidal::new(), TIDAL_EXPECTED);
    }
}
