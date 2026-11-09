//! Implementation of the FFT-based [YIN] pitch detection algorithm based on [pyin-rs].
//!
//! The original library, which is licensed under the MIT License (see below), has been simplified and modified to only use the YIN algorithm and to use f32 instead of f64.
//!
//! [YIN]: http://audition.ens.fr/adc/pdf/2002_JASA_YIN.pdf
//! [pyin-rs]: https://lib.rs/crates/pyin-rs

// MIT License
//
// Copyright (c) 2025 lucasworks
// Copyright (c) 2025 mebutkinky
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::ops::Range;

use std::sync::Arc;

use realfft::{ComplexToReal, RealFftPlanner, RealToComplex, num_complex::Complex32};

pub struct Yin {
    input_size: usize,
    sample_rate: usize,
    buffer: Vec<f32>,
    fft: FftContext,
    threshold: f32,
}

impl Yin {
    pub fn new(input_size: usize, sample_rate: usize, threshold: f32) -> Self {
        let buffer_size = input_size / 2;

        let fft = FftContext::new(input_size);

        Self {
            input_size,
            sample_rate,
            buffer: vec![0.0; buffer_size],
            fft,
            threshold,
        }
    }
}

impl Yin {
    /// Find the most significant fundamental pitch in the specified audio
    /// buffer, optionally within a given frequency range.
    /// Return 0.0 if no pitch is detected.
    pub fn pitch(&mut self, audio_buffer: &[f32], frequency_range: Option<Range<f32>>) -> f32 {
        self.preprocess(audio_buffer);

        let tau_range = self.calculate_tau_range(frequency_range);
        let tau_estimate = self.threshold(self.threshold, &tau_range);

        let result = if tau_estimate >= 0 {
            let x = self.parabolic_interpolation(tau_estimate as usize);
            self.sample_rate as f32 / x
        } else {
            0.0
        };

        self.fft.clear();
        result
    }
}

impl Yin {
    fn auto_correlate(&mut self, audio_buffer: &[f32]) {
        self.fft.buffer_in_mut().copy_from_slice(audio_buffer);
        self.fft.forward();

        let scale = 1.0 / self.fft.input_size() as f32;

        self.fft
            .buffer_out_mut()
            .iter_mut()
            .for_each(|c| *c *= c.conj() * scale);

        self.fft.inverse();
    }

    fn difference(&mut self, audio_buffer: &[f32]) {
        self.auto_correlate(audio_buffer);

        let fft = self.fft.buffer_in();
        for tau in 0..self.buffer.len() {
            self.buffer[tau] = fft[0] + fft[1] - 2.0 * fft[tau];
        }
    }

    /** Cumulative mean normalized difference. */
    fn cmnd(&mut self) {
        let mut running_sum = 0.0f32;

        self.buffer[0] = 1.0;

        for tau in 1..self.buffer.len() {
            running_sum += self.buffer[tau];
            self.buffer[tau] *= tau as f32 / running_sum;
        }
    }

    /** Take an input buffer, apply difference and CMND on it. */
    pub(crate) fn preprocess(&mut self, audio_buffer: &[f32]) {
        if audio_buffer.len() != self.input_size {
            panic!("audio buffer size mismatch");
        }

        self.difference(audio_buffer);
        self.cmnd();
    }

    /**
     * Calculate the range of tau, optionally based on a range of frequency.
     * The resulting tau range is always at least 2, and will not be larger
     * than the buffer size.
     */
    pub(crate) fn calculate_tau_range(&self, frequency_range: Option<Range<f32>>) -> Range<usize> {
        let buffer = &self.buffer;
        let size = buffer.len();

        let (min, max) = if let Some(range) = frequency_range {
            let sample_rate = self.sample_rate as f32;
            (
                ((sample_rate / range.end).floor() as usize).max(2),
                ((sample_rate / range.start).ceil() as usize).min(size),
            )
        } else {
            (2, size)
        };

        min..max
    }

    /**
     * Apply a threshold to the buffer, find the first local minimum tau
     * that is lower than the threshold.
     * Return -1 if no such tau is found.
     */
    pub(crate) fn threshold(&self, threshold: f32, tau_range: &Range<usize>) -> isize {
        let buffer = &self.buffer;

        let mut tau = tau_range.start;
        let max = tau_range.end;

        while tau < max {
            if buffer[tau] < threshold {
                while tau + 1 < max && buffer[tau + 1] < buffer[tau] {
                    tau += 1;
                }
                break;
            }
            tau += 1;
        }
        if tau == max || buffer[tau] >= threshold {
            -1
        } else {
            tau as isize
        }
    }

    pub(crate) fn parabolic_interpolation(&self, t: usize) -> f32 {
        let b = &self.buffer;
        if t < 1 {
            if b[t] <= b[t + 1] {
                t as f32
            } else {
                (t + 1) as f32
            }
        } else if t >= b.len() - 1 {
            (t - 1) as f32
        } else {
            let den = b[t + 1] + b[t - 1] - 2.0 * b[t];
            let delta = b[t - 1] - b[t + 1];
            if den == 0.0 {
                t as f32
            } else {
                // value is b[t] - delta * delta / (8.0 * den)
                t as f32 + delta / (2.0 * den)
            }
        }
    }
}

pub(crate) struct FftContext {
    fft_forward: Arc<dyn RealToComplex<f32>>,
    fft_inverse: Arc<dyn ComplexToReal<f32>>,
    buffer_in: Vec<f32>,
    buffer_out: Vec<Complex32>,
    scratch: Vec<Complex32>,
}

impl FftContext {
    pub fn new(size: usize) -> Self {
        if size == 0 || size & (size - 1) != 0 {
            panic!("power-of-two input size required. Got {}", size);
        }

        let mut real_planner = RealFftPlanner::<f32>::new();

        let fft_forward = real_planner.plan_fft_forward(size);
        let fft_inverse = real_planner.plan_fft_inverse(size);

        let buffer_in = fft_forward.make_input_vec();
        let buffer_out = fft_forward.make_output_vec();
        let scratch = fft_forward.make_scratch_vec();

        Self {
            fft_forward,
            fft_inverse,
            buffer_in,
            buffer_out,
            scratch,
        }
    }

    pub fn input_size(&self) -> usize {
        self.buffer_in.len()
    }

    pub fn buffer_in(&self) -> &[f32] {
        &self.buffer_in
    }

    pub fn buffer_in_mut(&mut self) -> &mut [f32] {
        &mut self.buffer_in
    }

    pub fn buffer_out_mut(&mut self) -> &mut [Complex32] {
        &mut self.buffer_out
    }

    pub fn clear(&mut self) {
        self.buffer_out
            .iter_mut()
            .for_each(|fft| *fft = Complex32::new(0.0, 0.0));
    }

    pub fn forward(&mut self) {
        self.fft_forward
            .process_with_scratch(&mut self.buffer_in, &mut self.buffer_out, &mut self.scratch)
            .unwrap();
    }

    pub fn inverse(&mut self) {
        self.fft_inverse
            .process_with_scratch(&mut self.buffer_out, &mut self.buffer_in, &mut self.scratch)
            .unwrap();
    }
}
