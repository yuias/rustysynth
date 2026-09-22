#![allow(dead_code)]

use std::f32::consts;

/// Number of channels in the Feedback Delay Network.
const FDN_SIZE: usize = 8;

/// 8x8 Hadamard FDN reverb with input diffusion and delay modulation.
///
/// Replaces Jezar's Freeverb with a higher-quality algorithm:
/// - 4 serial Schroeder all-pass diffusers for transient decorrelation
/// - 8-channel FDN with Hadamard mixing matrix (energy-preserving)
/// - Per-line feedback gain matched to a room-size-dependent target T60, so decay time
///   tracks Freeverb's regardless of each line's length (see `room_to_t60`)
/// - Per-channel 1-pole LP damping for frequency-dependent decay
/// - Sinusoidal delay modulation to suppress metallic ringing
/// - Stereo output via alternating channel taps
#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct Reverb {
    diffusers: Vec<AllPassDiffuser>,
    delay_lines: Vec<ModulatedDelayLine>,
    dampers: Vec<OnePoleLP>,

    sample_rate: f32,
    feedback: [f32; FDN_SIZE],
    damp_coeff: f32,
    input_gain: f32,
    wet: f32,
    wet1: f32,
    wet2: f32,
    width: f32,
    // `set_room_size` only stores derived per-line feedback gains, so the room size itself
    // needs a dedicated field for readback.
    room_size: f32,
}

impl Reverb {
    const SCALE_WET: f32 = 3.0;
    const SCALE_DAMP: f32 = 0.4;
    const SCALE_ROOM: f32 = 0.28;
    const OFFSET_ROOM: f32 = 0.7;
    const INITIAL_ROOM: f32 = 0.5;
    const INITIAL_DAMP: f32 = 0.5;
    pub(crate) const INITIAL_WET: f32 = 1.0 / Reverb::SCALE_WET;
    const INITIAL_WIDTH: f32 = 1.0;
    const INPUT_GAIN: f32 = 0.015;

    /// FDN delay line lengths at 44100 Hz (mutually prime for maximal mode density).
    const BASE_DELAYS: [usize; FDN_SIZE] = [1553, 1709, 1867, 2039, 2203, 2357, 2521, 2687];

    /// Per-channel sinusoidal modulation rates in Hz (varied for decorrelation).
    const MOD_RATES: [f32; FDN_SIZE] = [0.10, 0.15, 0.12, 0.18, 0.13, 0.17, 0.11, 0.16];

    /// Modulation depth in samples at 44100 Hz; scaled with sample rate like the delay lengths
    /// (see `scale_mod_depth`), otherwise the modulation becomes proportionally deeper relative
    /// to the delay length at low sample rates and shallower at high ones.
    const MOD_DEPTH: f32 = 8.0;

    /// Input diffusion all-pass delay lengths at 44100 Hz.
    const DIFFUSION_DELAYS: [usize; 4] = [142, 107, 379, 277];

    /// Input diffusion feedback coefficient.
    const DIFFUSION_COEFF: f32 = 0.75;

    /// 1/sqrt(8) for Hadamard normalization and input distribution.
    const NORM: f32 = 0.35355339;

    /// Numerator (in seconds) of the target-T60 curve fitted to Freeverb's room-size response,
    /// see `room_to_t60`.
    const T60_K: f32 = 0.092;

    /// Output gain, calibrated so the wet RMS for noise input matches the previous Freeverb
    /// at room size 0.5 / 44.1 kHz. Freeverb summed 8 parallel combs and its all-pass stages
    /// were not unity gain, so the FDN needs a large makeup gain.
    const OUTPUT_GAIN: f32 = 32.2;

    pub(crate) fn new(sample_rate: i32) -> Self {
        let sr_ratio = sample_rate as f64 / 44100.0;

        let mut diffusers = Vec::with_capacity(4);
        for &len in &Self::DIFFUSION_DELAYS {
            let scaled = Self::scale_delay(sr_ratio, len);
            diffusers.push(AllPassDiffuser::new(scaled, Self::DIFFUSION_COEFF));
        }

        let mod_depth = Self::scale_mod_depth(sr_ratio);
        let mut delay_lines = Vec::with_capacity(FDN_SIZE);
        for i in 0..FDN_SIZE {
            let length = Self::scale_delay(sr_ratio, Self::BASE_DELAYS[i]);
            let mod_rate = Self::MOD_RATES[i] * consts::TAU / sample_rate as f32;
            delay_lines.push(ModulatedDelayLine::new(length, mod_rate, mod_depth));
        }

        let mut dampers = Vec::with_capacity(FDN_SIZE);
        for _ in 0..FDN_SIZE {
            dampers.push(OnePoleLP::new());
        }

        let mut reverb = Reverb {
            diffusers,
            delay_lines,
            dampers,
            sample_rate: sample_rate as f32,
            feedback: [0.0; FDN_SIZE],
            damp_coeff: 0.0,
            input_gain: Self::INPUT_GAIN,
            wet: 0.0,
            wet1: 0.0,
            wet2: 0.0,
            width: 0.0,
            room_size: 0.0,
        };

        reverb.reset_params();

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

    fn scale_mod_depth(sr_ratio: f64) -> f32 {
        (Self::MOD_DEPTH as f64 * sr_ratio) as f32
    }

    /// Target T60 (seconds) for a room-size parameter. Follows the shape of Freeverb's comb
    /// decay time, which diverges as its feedback `room * SCALE_ROOM + OFFSET_ROOM` approaches 1.
    /// `T60_K` is fitted to Freeverb's measured RT60 because its damping keeps the real decay
    /// off the analytic per-comb formula; the result is within 10% for room sizes 0 to 1.
    fn room_to_t60(room: f32) -> f32 {
        let g = room * Self::SCALE_ROOM + Self::OFFSET_ROOM;
        Self::T60_K / -g.log10()
    }

    pub(crate) fn process(
        &mut self,
        input: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
    ) {
        output_left.fill(0.0);
        output_right.fill(0.0);

        for dl in &mut self.delay_lines {
            dl.begin_block();
        }

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

            // Feed back with decay, inject diffused input. Each line has its own feedback
            // gain (see `set_room_size`) so that all lines share the same T60 despite their
            // different lengths.
            let input_per_ch = diffused * Self::NORM;
            for i in 0..FDN_SIZE {
                self.delay_lines[i].write(tap[i] * self.feedback[i] + input_per_ch);
            }

            // Stereo output: alternating channel taps with sign variation for decorrelation
            let raw_l = (tap[0] + tap[2] - tap[4] + tap[6]) * Self::OUTPUT_GAIN;
            let raw_r = (tap[1] + tap[3] - tap[5] + tap[7]) * Self::OUTPUT_GAIN;

            output_left[t] = raw_l * self.wet1 + raw_r * self.wet2;
            output_right[t] = raw_r * self.wet1 + raw_l * self.wet2;
        }

        // Modulation is frozen for the duration of the block (see `begin_block`) and only
        // advanced afterward; at the modulation rates used here (0.1-0.18 Hz) the resulting
        // phase error within one block is negligible.
        for dl in &mut self.delay_lines {
            dl.advance_block(input.len());
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

    /// Restores the parameters a host or a GS reverb macro may have changed to the
    /// library defaults.
    pub(crate) fn reset_params(&mut self) {
        self.set_wet(Self::INITIAL_WET);
        self.set_room_size(Self::INITIAL_ROOM);
        self.set_damp(Self::INITIAL_DAMP);
        self.set_width(Self::INITIAL_WIDTH);
    }

    pub(crate) fn set_room_size(&mut self, value: f32) {
        self.room_size = value;
        // Standard T60-controlled FDN feedback (Jot): g_i = 10^(-3*L_i/(T60*fs)) makes every
        // line, regardless of its length, decay at exactly -60/T60 dB per second, so the mix
        // as a whole decays at the target T60.
        let t60 = Self::room_to_t60(value);
        for i in 0..FDN_SIZE {
            let len = self.delay_lines[i].base_length as f32;
            self.feedback[i] = 10.0_f32.powf(-3.0 * len / (t60 * self.sample_rate));
        }
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

    /// Last value passed to `set_room_size`.
    pub(crate) fn get_room_size(&self) -> f32 {
        self.room_size
    }

    /// Last value passed to `set_damp`, undoing the internal `SCALE_DAMP` scaling.
    pub(crate) fn get_damp(&self) -> f32 {
        self.damp_coeff / Self::SCALE_DAMP
    }

    /// Last value passed to `set_wet`, undoing the internal `SCALE_WET` scaling.
    pub(crate) fn get_wet(&self) -> f32 {
        self.wet / Self::SCALE_WET
    }

    /// Last value passed to `set_width`.
    pub(crate) fn get_width(&self) -> f32 {
        self.width
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
    // Read offset for the current block, computed once by `begin_block` instead of per sample.
    int_offset: usize,
    frac: f32,
}

impl ModulatedDelayLine {
    fn new(base_length: usize, mod_rate: f32, mod_depth: f32) -> Self {
        let buf_size = base_length + mod_depth.ceil() as usize + 2;
        Self {
            buffer: vec![0.0; buf_size],
            write_pos: 0,
            base_length,
            mod_phase: 0.0,
            mod_rate,
            mod_depth,
            int_offset: base_length,
            frac: 0.0,
        }
    }

    fn mute(&mut self) {
        self.buffer.fill(0.0);
    }

    /// Recompute the read offset once per block; modulation is far slower than the block
    /// rate, so freezing it within a block is inaudible and avoids a per-sample `sin()`.
    #[inline]
    fn begin_block(&mut self) {
        let offset = self.base_length as f32 + self.mod_depth * self.mod_phase.sin();
        self.int_offset = offset as usize;
        self.frac = offset - self.int_offset as f32;
    }

    /// Read with sinusoidal modulation and linear interpolation.
    #[inline]
    fn read(&self) -> f32 {
        let buf_len = self.buffer.len();

        let mut i0 = self.write_pos + buf_len - self.int_offset;
        if i0 >= buf_len {
            i0 -= buf_len;
        }
        let i1 = if i0 == 0 { buf_len - 1 } else { i0 - 1 };

        let mut val = self.buffer[i0] * (1.0 - self.frac) + self.buffer[i1] * self.frac;
        if val.abs() < 1.0e-20 {
            val = 0.0;
        }
        val
    }

    #[inline]
    fn write(&mut self, value: f32) {
        self.buffer[self.write_pos] = value;
        self.write_pos += 1;
        if self.write_pos == self.buffer.len() {
            self.write_pos = 0;
        }
    }

    /// Advance the modulation phase by one block's worth of samples.
    #[inline]
    fn advance_block(&mut self, block_len: usize) {
        self.mod_phase += self.mod_rate * block_len as f32;
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
        self.pos += 1;
        if self.pos == self.buffer.len() {
            self.pos = 0;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        }
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    /// Wet RMS for noise input at room 0.5 / 44.1 kHz, matching the level Freeverb produced
    /// under the same conditions (see reverb harness measurements: OUTPUT_GAIN calibration).
    /// This locks the calibrated level in place; a future change to OUTPUT_GAIN or the decay
    /// constants that shifts it out of this window should be a deliberate, re-measured choice.
    #[test]
    fn wet_rms_at_room_half_matches_calibrated_level() {
        let mut reverb = Reverb::new(44100);
        reverb.set_room_size(0.5);

        let block_size = 64;
        let mut rng = Lcg(1);
        let mut input = vec![0_f32; block_size];
        let mut left = vec![0_f32; block_size];
        let mut right = vec![0_f32; block_size];

        // Let the reverb tail build up before measuring, as a fresh delay network
        // starts under-energized relative to its steady-state response to noise.
        let warm_up_blocks = 3 * 44100 / block_size;
        for _ in 0..warm_up_blocks {
            for x in input.iter_mut() {
                *x = rng.next() * 0.015;
            }
            reverb.process(&input, &mut left, &mut right);
        }

        let measure_blocks = 3 * 44100 / block_size;
        let mut collected = Vec::with_capacity(measure_blocks * block_size);
        for _ in 0..measure_blocks {
            for x in input.iter_mut() {
                *x = rng.next() * 0.015;
            }
            reverb.process(&input, &mut left, &mut right);
            collected.extend_from_slice(&left);
        }

        let level = rms(&collected);
        assert!(
            (0.17..0.21).contains(&level),
            "wet RMS {} is outside the calibrated window",
            level
        );
    }

    /// At room size 0, the reverb should decay quickly and stay numerically well-behaved.
    #[test]
    fn room_zero_decays_and_stays_finite() {
        for &sample_rate in &[16000, 192000] {
            let mut reverb = Reverb::new(sample_rate);
            reverb.set_room_size(0.0);

            let block_size = 64;
            let mut rng = Lcg(2);
            let mut input = vec![0_f32; block_size];
            let mut left = vec![0_f32; block_size];
            let mut right = vec![0_f32; block_size];

            let noise_blocks = sample_rate as usize / block_size;
            for _ in 0..noise_blocks {
                for x in input.iter_mut() {
                    *x = rng.next() * 0.015;
                }
                reverb.process(&input, &mut left, &mut right);
                assert!(left.iter().chain(right.iter()).all(|x| x.is_finite()));
            }

            // Room 0's T60 is under a second; a few seconds of silence should bring
            // the tail down to near-silence.
            let silence = vec![0_f32; block_size];
            let silence_blocks = 3 * sample_rate as usize / block_size;
            for i in 0..silence_blocks {
                reverb.process(&silence, &mut left, &mut right);
                assert!(left.iter().chain(right.iter()).all(|x| x.is_finite()));
                if i == silence_blocks - 1 {
                    let peak = left
                        .iter()
                        .chain(right.iter())
                        .fold(0_f32, |m, x| m.max(x.abs()));
                    assert!(peak < 1.0e-4, "peak {} did not decay near silence", peak);
                }
            }
        }
    }

    /// After noise stops, the decaying tail should eventually flush to exact zero (or
    /// below the denormal-flush threshold), confirming the 1e-20 flush inside the delay
    /// lines, dampers and diffusers actually reaches the output.
    #[test]
    fn silence_after_noise_flushes_denormals() {
        let mut reverb = Reverb::new(44100);
        reverb.set_room_size(0.5);

        let block_size = 64;
        let mut rng = Lcg(3);
        let mut input = vec![0_f32; block_size];
        let mut left = vec![0_f32; block_size];
        let mut right = vec![0_f32; block_size];

        let noise_blocks = 44100 / block_size;
        for _ in 0..noise_blocks {
            for x in input.iter_mut() {
                *x = rng.next() * 0.015;
            }
            reverb.process(&input, &mut left, &mut right);
        }

        let silence = vec![0_f32; block_size];
        // Room 0.5's T60 is ~1.1 s; 15 s of silence is more than an order of magnitude
        // of decay time, well past where the internal 1e-20 flush should have zeroed
        // every line.
        let silence_blocks = 15 * 44100 / block_size;
        for _ in 0..silence_blocks {
            reverb.process(&silence, &mut left, &mut right);
        }

        assert!(left
            .iter()
            .chain(right.iter())
            .all(|&x| x == 0.0 || x.abs() < 1.0e-15));
    }
}
