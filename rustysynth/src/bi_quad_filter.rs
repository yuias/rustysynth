#![allow(dead_code)]

use std::f32::consts;

use crate::synthesizer_settings::SynthesizerSettings;

/// Topology-Preserving Transform State Variable Filter (Cytomic TPT SVF).
///
/// Replaces the Direct Form I biquad with an integrator-based design that is
/// inherently stable under rapid parameter modulation. Two integrator states
/// (`ic1eq`, `ic2eq`) replace the four delay states of the biquad.
///
/// Reference: Andrew Simper, "SvfLinearTrapOptimised2" (Cytomic).
#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct BiQuadFilter {
    sample_rate: i32,

    active: bool,

    // SVF coefficients (precomputed per set_low_pass_filter call)
    a1: f32,
    a2: f32,
    a3: f32,

    // Integrator states
    ic1eq: f32,
    ic2eq: f32,
}

impl BiQuadFilter {
    const RESONANCE_PEAK_OFFSET: f32 = 1_f32 - 1_f32 / core::f32::consts::SQRT_2;
    const MIN_Q: f32 = 0.5;

    pub(crate) fn new(settings: &SynthesizerSettings) -> Self {
        Self {
            sample_rate: settings.sample_rate,
            active: false,
            a1: 0_f32,
            a2: 0_f32,
            a3: 0_f32,
            ic1eq: 0_f32,
            ic2eq: 0_f32,
        }
    }

    pub(crate) fn clear_buffer(&mut self) {
        self.ic1eq = 0_f32;
        self.ic2eq = 0_f32;
    }

    /// `resonance` is the linear SF2 filter Q; `q_scale` multiplies the resulting Q
    /// (used for the channel resonance controller, 1.0 = no change).
    pub(crate) fn set_low_pass_filter(&mut self, cutoff_frequency: f32, resonance: f32, q_scale: f32) {
        if cutoff_frequency < 0.499_f32 * self.sample_rate as f32 {
            self.active = true;

            let q = BiQuadFilter::resonance_to_q(resonance, q_scale);

            // TPT SVF coefficients
            let g = (consts::PI * cutoff_frequency / self.sample_rate as f32).tan();
            let k = 1.0 / q;
            self.a1 = 1.0 / (1.0 + g * (g + k));
            self.a2 = g * self.a1;
            self.a3 = g * self.a2;
        } else {
            self.active = false;
        }
    }

    fn resonance_to_q(resonance: f32, q_scale: f32) -> f32 {
        // The peak-offset formula has a pole at resonance = 5/6 and is only meaningful
        // for SF2 filter Q >= 0 dB, so any attenuation is applied in the Q domain instead.
        let resonance = resonance.max(1_f32);
        let q = resonance
            - BiQuadFilter::RESONANCE_PEAK_OFFSET / (1_f32 + 6_f32 * (resonance - 1_f32));
        (q * q_scale).max(BiQuadFilter::MIN_Q)
    }

    pub(crate) fn process(&mut self, block: &mut [f32]) {
        if self.active {
            for input in block.iter_mut() {
                let v3 = *input - self.ic2eq;
                let v1 = self.a1 * self.ic1eq + self.a2 * v3;
                let v2 = self.ic2eq + self.a2 * self.ic1eq + self.a3 * v3;
                self.ic1eq = 2.0 * v1 - self.ic1eq;
                self.ic2eq = 2.0 * v2 - self.ic2eq;

                *input = v2; // LP output
            }

            // Flush denormals
            if self.ic1eq.abs() < 1.0e-20 {
                self.ic1eq = 0.0;
            }
            if self.ic2eq.abs() < 1.0e-20 {
                self.ic2eq = 0.0;
            }
        } else {
            // Track signal for smooth transition when filter becomes active.
            // At DC steady state: ic1eq=0 (BP=0), ic2eq=input (LP=input).
            let block_length = block.len();
            self.ic1eq = 0_f32;
            self.ic2eq = block[block_length - 1];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resonance_controller_changes_q_monotonically() {
        // SF2 filter Q of 0 dB, swept over CC#71 values at 0.5 dB per step.
        let mut previous = 0_f32;
        for cc in 0..128 {
            let q_scale = 10_f32.powf((cc as f32 - 64.0) * 0.5 / 20.0);
            let q = BiQuadFilter::resonance_to_q(1.0, q_scale);
            assert!(q >= BiQuadFilter::MIN_Q, "cc = {}: q = {}", cc, q);
            assert!(q >= previous, "cc = {}: q = {} < {}", cc, q, previous);
            previous = q;
        }
    }

    #[test]
    fn neutral_q_scale_keeps_sf2_mapping() {
        let q = BiQuadFilter::resonance_to_q(1.0, 1.0);
        assert!((q - core::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }
}
