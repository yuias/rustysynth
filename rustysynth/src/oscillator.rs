#![allow(dead_code)]

use crate::loop_mode::LoopMode;
use crate::synthesizer_settings::SynthesizerSettings;

// In this class, fixed-point numbers are used for speed-up.
// A fixed-point number is expressed by Int64, whose lower 24 bits represent the fraction part,
// and the rest represent the integer part.
// For clarity, fixed-point number variables have a suffix "_fp".

#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct Oscillator {
    synthesizer_sample_rate: i32,

    loop_mode: LoopMode,
    sample_sample_rate: i32,
    start: i32,
    end: i32,
    start_loop: i32,
    end_loop: i32,
    root_key: i32,

    tune: f32,
    pitch_change_scale: f32,
    sample_rate_ratio: f32,

    looping: bool,

    position_fp: i64,
}

impl Oscillator {
    const FRAC_BITS: i32 = 24;
    const FRAC_UNIT: i64 = 1_i64 << Oscillator::FRAC_BITS;
    const FRAC_UNIT_RECIP: f64 = 1.0 / (1_i64 << 24) as f64;
    const SAMPLE_RECIP: f64 = 1.0 / 32768.0;

    pub(crate) fn new(settings: &SynthesizerSettings) -> Self {
        Self {
            synthesizer_sample_rate: settings.sample_rate,
            loop_mode: LoopMode::NoLoop,
            sample_sample_rate: 0,
            start: 0,
            end: 0,
            start_loop: 0,
            end_loop: 0,
            root_key: 0,
            tune: 0_f32,
            pitch_change_scale: 0_f32,
            sample_rate_ratio: 0_f32,
            looping: false,
            position_fp: 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        &mut self,
        loop_mode: LoopMode,
        sample_rate: i32,
        start: i32,
        end: i32,
        start_loop: i32,
        end_loop: i32,
        root_key: i32,
        coarse_tune: i32,
        fine_tune: i32,
        scale_tuning: i32,
    ) {
        self.loop_mode = loop_mode;
        self.sample_sample_rate = sample_rate;
        self.start = start;
        self.end = end;
        self.start_loop = start_loop;
        self.end_loop = end_loop;
        self.root_key = root_key;

        self.tune = coarse_tune as f32 + 0.01_f32 * fine_tune as f32;
        self.pitch_change_scale = 0.01_f32 * scale_tuning as f32;
        self.sample_rate_ratio = sample_rate as f32 / self.synthesizer_sample_rate as f32;
        self.looping = self.loop_mode != LoopMode::NoLoop;
        self.position_fp = (start as i64) << Oscillator::FRAC_BITS;
    }

    pub(crate) fn release(&mut self) {
        if self.loop_mode == LoopMode::LoopUntilNoteOff {
            self.looping = false;
        }
    }

    pub(crate) fn process(&mut self, data: &[i16], block: &mut [f32], pitch: f32) -> bool {
        let pitch_change = self.pitch_change_scale * (pitch - self.root_key as f32) + self.tune;
        let pitch_ratio = self.sample_rate_ratio * 2_f32.powf(pitch_change / 12_f32);
        self.fill_block(data, block, pitch_ratio as f64)
    }

    fn fill_block(&mut self, data: &[i16], block: &mut [f32], pitch_ratio: f64) -> bool {
        let pitch_ratio_fp = (Oscillator::FRAC_UNIT as f64 * pitch_ratio) as i64;

        if self.looping {
            self.fill_block_continuous(data, block, pitch_ratio_fp)
        } else {
            self.fill_block_no_loop(data, block, pitch_ratio_fp)
        }
    }

    /// 4-point Hermite interpolation for non-looping samples.
    /// Boundary handling: x0 is clamped at sample start; x3 relies on wave_data padding.
    fn fill_block_no_loop(&mut self, data: &[i16], block: &mut [f32], pitch_ratio_fp: i64) -> bool {
        let start = self.start as usize;
        let end = self.end as usize;

        for t in 0..block.len() {
            let index = (self.position_fp >> Oscillator::FRAC_BITS) as usize;
            if index >= end {
                if t > 0 {
                    block[t..].fill(0_f32);
                    return true;
                } else {
                    return false;
                }
            }

            let i0 = if index > start { index - 1 } else { start };

            let x0 = data[i0] as f64;
            let x1 = data[index] as f64;
            let x2 = data[index + 1] as f64;
            let x3 = data[index + 2] as f64;

            let frac = (self.position_fp & (Oscillator::FRAC_UNIT - 1)) as f64
                * Oscillator::FRAC_UNIT_RECIP;

            block[t] = Self::hermite(x0, x1, x2, x3, frac);
            self.position_fp += pitch_ratio_fp;
        }

        true
    }

    /// 4-point Hermite interpolation for looping samples.
    /// All indices wrap within the loop region [start_loop, end_loop).
    fn fill_block_continuous(
        &mut self,
        data: &[i16],
        block: &mut [f32],
        pitch_ratio_fp: i64,
    ) -> bool {
        let end_loop_fp = (self.end_loop as i64) << Oscillator::FRAC_BITS;
        let loop_length = (self.end_loop - self.start_loop) as i64;
        let loop_length_fp = loop_length << Oscillator::FRAC_BITS;
        let sl = self.start_loop as usize;
        let el = self.end_loop as usize;
        let ll = loop_length as usize;

        for sample in block.iter_mut() {
            if self.position_fp >= end_loop_fp {
                self.position_fp -= loop_length_fp;
            }

            let index = (self.position_fp >> Oscillator::FRAC_BITS) as usize;

            let i0 = if index > sl { index - 1 } else { el - 1 };
            let mut i2 = index + 1;
            if i2 >= el { i2 -= ll; }
            let mut i3 = index + 2;
            if i3 >= el { i3 -= ll; }
            if i3 >= el { i3 -= ll; } // handles degenerate loop_length == 1

            let x0 = data[i0] as f64;
            let x1 = data[index] as f64;
            let x2 = data[i2] as f64;
            let x3 = data[i3] as f64;

            let frac = (self.position_fp & (Oscillator::FRAC_UNIT - 1)) as f64
                * Oscillator::FRAC_UNIT_RECIP;

            *sample = Self::hermite(x0, x1, x2, x3, frac);
            self.position_fp += pitch_ratio_fp;
        }

        true
    }

    #[inline(always)]
    fn hermite(x0: f64, x1: f64, x2: f64, x3: f64, frac: f64) -> f32 {
        let c1 = 0.5 * (x2 - x0);
        let c2 = x0 - 2.5 * x1 + 2.0 * x2 - 0.5 * x3;
        let c3 = 0.5 * (x3 - x0) + 1.5 * (x1 - x2);
        (((c3 * frac + c2) * frac + c1) * frac + x1) as f32 * Self::SAMPLE_RECIP as f32
    }
}
