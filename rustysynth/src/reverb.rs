#![allow(dead_code)]

use std::f32::consts;

/// Number of channels in the Feedback Delay Network.
const FDN_SIZE: usize = 8;

/// 8x8 Hadamard FDN reverb with input diffusion and delay modulation.
///
/// Replaces Jezar's Freeverb with a higher-quality algorithm:
/// - 4 serial Schroeder all-pass diffusers for transient decorrelation
/// - 8-channel FDN with Hadamard mixing matrix (energy-preserving)
/// - Per-channel 1-pole LP damping for frequency-dependent decay
/// - Sinusoidal delay modulation to suppress metallic ringing
/// - Stereo output via alternating channel taps
#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct Reverb {
    diffusers: Vec<AllPassDiffuser>,
    delay_lines: Vec<ModulatedDelayLine>,
    dampers: Vec<OnePoleLP>,

    feedback: f32,
    damp_coeff: f32,
    input_gain: f32,
    wet: f32,
    wet1: f32,
    wet2: f32,
    width: f32,
}

impl Reverb {
    const SCALE_WET: f32 = 3.0;
    const SCALE_DAMP: f32 = 0.4;
    const SCALE_ROOM: f32 = 0.28;
    const OFFSET_ROOM: f32 = 0.7;
    const INITIAL_ROOM: f32 = 0.5;
    const INITIAL_DAMP: f32 = 0.5;
    const INITIAL_WET: f32 = 1.0 / Reverb::SCALE_WET;
    const INITIAL_WIDTH: f32 = 1.0;
    const INPUT_GAIN: f32 = 0.015;

    /// FDN delay line lengths at 44100 Hz (mutually prime for maximal mode density).
    const BASE_DELAYS: [usize; FDN_SIZE] = [1553, 1709, 1867, 2039, 2203, 2357, 2521, 2687];

    /// Per-channel sinusoidal modulation rates in Hz (varied for decorrelation).
    const MOD_RATES: [f32; FDN_SIZE] = [0.10, 0.15, 0.12, 0.18, 0.13, 0.17, 0.11, 0.16];

    /// Modulation depth in samples.
    const MOD_DEPTH: f32 = 8.0;

    /// Input diffusion all-pass delay lengths at 44100 Hz.
    const DIFFUSION_DELAYS: [usize; 4] = [142, 107, 379, 277];

    /// Input diffusion feedback coefficient.
    const DIFFUSION_COEFF: f32 = 0.75;

    /// 1/sqrt(8) for Hadamard normalization and input distribution.
    const NORM: f32 = 0.35355339;

    /// Output gain compensation (FDN produces lower amplitude than Freeverb
    /// due to energy distribution across channels).
    const OUTPUT_GAIN: f32 = 2.0;

    pub(crate) fn new(sample_rate: i32) -> Self {
        let sr_ratio = sample_rate as f64 / 44100.0;

        let mut diffusers = Vec::with_capacity(4);
        for &len in &Self::DIFFUSION_DELAYS {
            let scaled = Self::scale_delay(sr_ratio, len);
            diffusers.push(AllPassDiffuser::new(scaled, Self::DIFFUSION_COEFF));
        }

        let mut delay_lines = Vec::with_capacity(FDN_SIZE);
        for i in 0..FDN_SIZE {
            let length = Self::scale_delay(sr_ratio, Self::BASE_DELAYS[i]);
            let mod_rate = Self::MOD_RATES[i] * consts::TAU / sample_rate as f32;
            delay_lines.push(ModulatedDelayLine::new(length, mod_rate, Self::MOD_DEPTH));
        }

        let mut dampers = Vec::with_capacity(FDN_SIZE);
        for _ in 0..FDN_SIZE {
            dampers.push(OnePoleLP::new());
        }

        let mut reverb = Reverb {
            diffusers,
            delay_lines,
            dampers,
            feedback: 0.0,
            damp_coeff: 0.0,
            input_gain: Self::INPUT_GAIN,
            wet: 0.0,
            wet1: 0.0,
            wet2: 0.0,
            width: 0.0,
        };

        reverb.set_wet(Self::INITIAL_WET);
        reverb.set_room_size(Self::INITIAL_ROOM);
        reverb.set_damp(Self::INITIAL_DAMP);
        reverb.set_width(Self::INITIAL_WIDTH);

        reverb
    }

    pub fn mute(&mut self) {
        for d in &mut self.diffusers {
            d.mute();
        }
        for dl in &mut self.delay_lines {
            dl.mute();
        }
        for lp in &mut self.dampers {
            lp.reset();
        }
    }

    fn scale_delay(sr_ratio: f64, base: usize) -> usize {
        (sr_ratio * base as f64).round() as usize
    }

    pub(crate) fn process(
        &mut self,
        input: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
    ) {
        output_left.fill(0.0);
        output_right.fill(0.0);

        for t in 0..input.len() {
            // Input diffusion: decorrelate transients through serial all-pass chain
            let mut diffused = input[t];
            for d in &mut self.diffusers {
                diffused = d.process(diffused);
            }

            // Read from FDN delay lines with modulated read positions
            let mut tap = [0.0_f32; FDN_SIZE];
            for i in 0..FDN_SIZE {
                tap[i] = self.delay_lines[i].read();
            }

            // Frequency-dependent damping (1-pole LP per channel)
            for i in 0..FDN_SIZE {
                tap[i] = self.dampers[i].process(tap[i], self.damp_coeff);
            }

            // Hadamard mixing (energy-preserving, O(N log N) butterfly)
            Self::hadamard8(&mut tap);

            // Feed back with decay, inject diffused input
            let input_per_ch = diffused * Self::NORM;
            for i in 0..FDN_SIZE {
                self.delay_lines[i].write(tap[i] * self.feedback + input_per_ch);
                self.delay_lines[i].advance_mod();
            }

            // Stereo output: alternating channel taps with sign variation for decorrelation
            let raw_l = (tap[0] + tap[2] - tap[4] + tap[6]) * Self::OUTPUT_GAIN;
            let raw_r = (tap[1] + tap[3] - tap[5] + tap[7]) * Self::OUTPUT_GAIN;

            output_left[t] = raw_l * self.wet1 + raw_r * self.wet2;
            output_right[t] = raw_r * self.wet1 + raw_l * self.wet2;
        }
    }

    /// In-place 8-point Walsh-Hadamard transform, normalized by 1/sqrt(8).
    #[inline]
    fn hadamard8(x: &mut [f32; FDN_SIZE]) {
        // Stage 1: butterfly stride 1
        for i in (0..8).step_by(2) {
            let a = x[i];
            let b = x[i + 1];
            x[i] = a + b;
            x[i + 1] = a - b;
        }
        // Stage 2: butterfly stride 2
        for i in (0..8).step_by(4) {
            for j in 0..2 {
                let a = x[i + j];
                let b = x[i + j + 2];
                x[i + j] = a + b;
                x[i + j + 2] = a - b;
            }
        }
        // Stage 3: butterfly stride 4
        for i in 0..4 {
            let a = x[i];
            let b = x[i + 4];
            x[i] = a + b;
            x[i + 4] = a - b;
        }
        // Normalize
        for v in x.iter_mut() {
            *v *= Self::NORM;
        }
    }

    pub fn get_input_gain(&self) -> f32 {
        self.input_gain
    }

    pub(crate) fn set_room_size(&mut self, value: f32) {
        self.feedback = value * Self::SCALE_ROOM + Self::OFFSET_ROOM;
    }

    pub(crate) fn set_damp(&mut self, value: f32) {
        self.damp_coeff = value * Self::SCALE_DAMP;
    }

    pub(crate) fn set_wet(&mut self, value: f32) {
        self.wet = value * Self::SCALE_WET;
        self.update_wet();
    }

    pub(crate) fn set_width(&mut self, value: f32) {
        self.width = value;
        self.update_wet();
    }

    fn update_wet(&mut self) {
        self.wet1 = self.wet * (self.width / 2.0 + 0.5);
        self.wet2 = self.wet * ((1.0 - self.width) / 2.0);
    }
}

// ---------------------------------------------------------------------------
// Modulated Delay Line
// ---------------------------------------------------------------------------

/// Circular buffer delay line with sinusoidal read-position modulation.
/// Modulation prevents metallic ringing on long reverb tails.
#[derive(Debug)]
struct ModulatedDelayLine {
    buffer: Vec<f32>,
    write_pos: usize,
    base_length: usize,
    mod_phase: f32,
    mod_rate: f32,
    mod_depth: f32,
}

impl ModulatedDelayLine {
    fn new(base_length: usize, mod_rate: f32, mod_depth: f32) -> Self {
        let buf_size = base_length + (mod_depth as usize) + 2;
        Self {
            buffer: vec![0.0; buf_size],
            write_pos: 0,
            base_length,
            mod_phase: 0.0,
            mod_rate,
            mod_depth,
        }
    }

    fn mute(&mut self) {
        self.buffer.fill(0.0);
    }

    /// Read with sinusoidal modulation and linear interpolation.
    #[inline]
    fn read(&self) -> f32 {
        let offset = self.base_length as f32 + self.mod_depth * self.mod_phase.sin();
        let int_offset = offset as usize;
        let frac = offset - int_offset as f32;

        let buf_len = self.buffer.len();
        let i0 = (self.write_pos + buf_len - int_offset) % buf_len;
        let i1 = (self.write_pos + buf_len - int_offset - 1) % buf_len;

        let mut val = self.buffer[i0] * (1.0 - frac) + self.buffer[i1] * frac;
        if val.abs() < 1.0e-20 {
            val = 0.0;
        }
        val
    }

    #[inline]
    fn write(&mut self, value: f32) {
        self.buffer[self.write_pos] = value;
        self.write_pos = (self.write_pos + 1) % self.buffer.len();
    }

    #[inline]
    fn advance_mod(&mut self) {
        self.mod_phase += self.mod_rate;
        if self.mod_phase >= consts::TAU {
            self.mod_phase -= consts::TAU;
        }
    }
}

// ---------------------------------------------------------------------------
// Schroeder All-Pass Diffuser
// ---------------------------------------------------------------------------

/// True Schroeder all-pass filter: H(z) = (g + z^-D) / (1 + g*z^-D).
/// Used for input diffusion to decorrelate transient onsets.
#[derive(Debug)]
struct AllPassDiffuser {
    buffer: Vec<f32>,
    pos: usize,
    feedback: f32,
}

impl AllPassDiffuser {
    fn new(length: usize, feedback: f32) -> Self {
        Self {
            buffer: vec![0.0; length.max(1)],
            pos: 0,
            feedback,
        }
    }

    fn mute(&mut self) {
        self.buffer.fill(0.0);
    }

    /// Process one sample through the all-pass.
    /// w[n] = x[n] - g * w[n-D];  y[n] = g * w[n] + w[n-D]
    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let mut delayed = self.buffer[self.pos];
        if delayed.abs() < 1.0e-20 {
            delayed = 0.0;
        }

        let w = input - self.feedback * delayed;
        let output = self.feedback * w + delayed;
        self.buffer[self.pos] = w;
        self.pos = (self.pos + 1) % self.buffer.len();
        output
    }
}

// ---------------------------------------------------------------------------
// One-Pole Low-Pass Filter
// ---------------------------------------------------------------------------

/// Simple 1-pole LP for frequency-dependent decay: y[n] = (1-d)*x[n] + d*y[n-1].
#[derive(Debug)]
struct OnePoleLP {
    state: f32,
}

impl OnePoleLP {
    fn new() -> Self {
        Self { state: 0.0 }
    }

    fn reset(&mut self) {
        self.state = 0.0;
    }

    #[inline]
    fn process(&mut self, input: f32, damp: f32) -> f32 {
        self.state = input * (1.0 - damp) + self.state * damp;
        if self.state.abs() < 1.0e-20 {
            self.state = 0.0;
        }
        self.state
    }
}
