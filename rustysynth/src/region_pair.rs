#![allow(dead_code)]

use crate::generator_type::GeneratorType;
use crate::instrument_region::InstrumentRegion;
use crate::loop_mode::LoopMode;
use crate::preset_region::PresetRegion;
use crate::soundfont_math::SoundFontMath;
use crate::voice_modulators::GeneratorOffsets;

/// The useful value range the SoundFont specification gives a generator in its generator
/// summary, or `None` for the generators it leaves unbounded or describes with flags and
/// sentinel values.
///
/// The sum of the preset and instrument values, plus any modulator offset fixed at note-on,
/// can leave that range. The specification calls for substituting the nearest realizable
/// value rather than using the sum as it stands.
///
/// Coarse and fine tune are deliberately absent: the pitch of a sounding note is offset
/// outside the region, so clamping here would not bound it anyway, and the pitch wheel
/// needs far more than the 99 cents the fine tune generator allows.
fn generator_range(generator: usize) -> Option<(i32, i32)> {
    match generator as u16 {
        GeneratorType::MODULATION_LFO_TO_PITCH => Some((-12000, 12000)),
        GeneratorType::VIBRATO_LFO_TO_PITCH => Some((-12000, 12000)),
        GeneratorType::MODULATION_ENVELOPE_TO_PITCH => Some((-12000, 12000)),
        GeneratorType::INITIAL_FILTER_CUTOFF_FREQUENCY => Some((1500, 13500)),
        GeneratorType::INITIAL_FILTER_Q => Some((0, 960)),
        GeneratorType::MODULATION_LFO_TO_FILTER_CUTOFF_FREQUENCY => Some((-12000, 12000)),
        GeneratorType::MODULATION_ENVELOPE_TO_FILTER_CUTOFF_FREQUENCY => Some((-12000, 12000)),
        GeneratorType::MODULATION_LFO_TO_VOLUME => Some((-960, 960)),
        GeneratorType::CHORUS_EFFECTS_SEND => Some((0, 1000)),
        GeneratorType::REVERB_EFFECTS_SEND => Some((0, 1000)),
        GeneratorType::PAN => Some((-500, 500)),
        GeneratorType::DELAY_MODULATION_LFO => Some((-12000, 5000)),
        GeneratorType::FREQUENCY_MODULATION_LFO => Some((-16000, 4500)),
        GeneratorType::DELAY_VIBRATO_LFO => Some((-12000, 5000)),
        GeneratorType::FREQUENCY_VIBRATO_LFO => Some((-16000, 4500)),
        GeneratorType::DELAY_MODULATION_ENVELOPE => Some((-12000, 5000)),
        GeneratorType::ATTACK_MODULATION_ENVELOPE => Some((-12000, 8000)),
        GeneratorType::HOLD_MODULATION_ENVELOPE => Some((-12000, 5000)),
        GeneratorType::DECAY_MODULATION_ENVELOPE => Some((-12000, 8000)),
        GeneratorType::SUSTAIN_MODULATION_ENVELOPE => Some((0, 1000)),
        GeneratorType::RELEASE_MODULATION_ENVELOPE => Some((-12000, 8000)),
        GeneratorType::KEY_NUMBER_TO_MODULATION_ENVELOPE_HOLD => Some((-1200, 1200)),
        GeneratorType::KEY_NUMBER_TO_MODULATION_ENVELOPE_DECAY => Some((-1200, 1200)),
        GeneratorType::DELAY_VOLUME_ENVELOPE => Some((-12000, 5000)),
        GeneratorType::ATTACK_VOLUME_ENVELOPE => Some((-12000, 8000)),
        GeneratorType::HOLD_VOLUME_ENVELOPE => Some((-12000, 5000)),
        GeneratorType::DECAY_VOLUME_ENVELOPE => Some((-12000, 8000)),
        GeneratorType::SUSTAIN_VOLUME_ENVELOPE => Some((0, 1440)),
        GeneratorType::RELEASE_VOLUME_ENVELOPE => Some((-12000, 8000)),
        GeneratorType::KEY_NUMBER_TO_VOLUME_ENVELOPE_HOLD => Some((-1200, 1200)),
        GeneratorType::KEY_NUMBER_TO_VOLUME_ENVELOPE_DECAY => Some((-1200, 1200)),
        GeneratorType::INITIAL_ATTENUATION => Some((0, 1440)),
        GeneratorType::SCALE_TUNING => Some((0, 1200)),
        _ => None,
    }
}

#[non_exhaustive]
pub(crate) struct RegionPair<'a> {
    pub(crate) preset: &'a PresetRegion,
    pub(crate) instrument: &'a InstrumentRegion,
    offsets: Option<&'a GeneratorOffsets>,
    clamp_to_range: bool,
}

impl<'a> RegionPair<'a> {
    pub(crate) fn new(
        preset: &'a PresetRegion,
        instrument: &'a InstrumentRegion,
        clamp_to_range: bool,
    ) -> Self {
        Self {
            preset,
            instrument,
            offsets: None,
            clamp_to_range,
        }
    }

    /// Returns the same region pair with modulator offsets added to every generator.
    pub(crate) fn with_offsets(&self, offsets: &'a GeneratorOffsets) -> RegionPair<'a> {
        Self {
            preset: self.preset,
            instrument: self.instrument,
            offsets: Some(offsets),
            clamp_to_range: self.clamp_to_range,
        }
    }

    fn gs(&self, i: usize) -> i32 {
        let value = self.preset.gs[i] as i32 + self.instrument.gs[i] as i32;
        let value = match self.offsets {
            Some(offsets) => value + offsets[i].round() as i32,
            None => value,
        };
        match generator_range(i).filter(|_| self.clamp_to_range) {
            Some((low, high)) => value.clamp(low, high),
            None => value,
        }
    }

    pub(crate) fn get_sample_start(&self) -> i32 {
        self.instrument.get_sample_start()
    }

    pub(crate) fn get_sample_end(&self) -> i32 {
        self.instrument.get_sample_end()
    }

    pub(crate) fn get_sample_start_loop(&self) -> i32 {
        self.instrument.get_sample_start_loop()
    }

    pub(crate) fn get_sample_end_loop(&self) -> i32 {
        self.instrument.get_sample_end_loop()
    }

    pub(crate) fn get_start_address_offset(&self) -> i32 {
        self.instrument.get_start_address_offset()
    }

    pub(crate) fn get_end_address_offset(&self) -> i32 {
        self.instrument.get_end_address_offset()
    }

    pub(crate) fn get_start_loop_address_offset(&self) -> i32 {
        self.instrument.get_start_loop_address_offset()
    }

    pub(crate) fn get_end_loop_address_offset(&self) -> i32 {
        self.instrument.get_end_loop_address_offset()
    }

    pub(crate) fn get_modulation_lfo_to_pitch(&self) -> i32 {
        self.gs(GeneratorType::MODULATION_LFO_TO_PITCH as usize)
    }

    pub(crate) fn get_vibrato_lfo_to_pitch(&self) -> i32 {
        self.gs(GeneratorType::VIBRATO_LFO_TO_PITCH as usize)
    }

    pub(crate) fn get_modulation_envelope_to_pitch(&self) -> i32 {
        self.gs(GeneratorType::MODULATION_ENVELOPE_TO_PITCH as usize)
    }

    pub(crate) fn get_initial_filter_cutoff_frequency(&self) -> f32 {
        SoundFontMath::cents_to_hertz(
            self.gs(GeneratorType::INITIAL_FILTER_CUTOFF_FREQUENCY as usize) as f32,
        )
    }

    pub(crate) fn get_initial_filter_q(&self) -> f32 {
        0.1_f32 * self.gs(GeneratorType::INITIAL_FILTER_Q as usize) as f32
    }

    pub(crate) fn get_modulation_lfo_to_filter_cutoff_frequency(&self) -> i32 {
        self.gs(GeneratorType::MODULATION_LFO_TO_FILTER_CUTOFF_FREQUENCY as usize)
    }

    pub(crate) fn get_modulation_envelope_to_filter_cutoff_frequency(&self) -> i32 {
        self.gs(GeneratorType::MODULATION_ENVELOPE_TO_FILTER_CUTOFF_FREQUENCY as usize)
    }

    pub(crate) fn get_modulation_lfo_to_volume(&self) -> f32 {
        0.1_f32 * self.gs(GeneratorType::MODULATION_LFO_TO_VOLUME as usize) as f32
    }

    pub(crate) fn get_chorus_effects_send(&self) -> f32 {
        0.1_f32 * self.gs(GeneratorType::CHORUS_EFFECTS_SEND as usize) as f32
    }

    pub(crate) fn get_reverb_effects_send(&self) -> f32 {
        0.1_f32 * self.gs(GeneratorType::REVERB_EFFECTS_SEND as usize) as f32
    }

    pub(crate) fn get_pan(&self) -> f32 {
        0.1_f32 * self.gs(GeneratorType::PAN as usize) as f32
    }

    pub(crate) fn get_delay_modulation_lfo(&self) -> f32 {
        SoundFontMath::timecents_to_seconds(
            self.gs(GeneratorType::DELAY_MODULATION_LFO as usize) as f32
        )
    }

    pub(crate) fn get_frequency_modulation_lfo(&self) -> f32 {
        SoundFontMath::cents_to_hertz(
            self.gs(GeneratorType::FREQUENCY_MODULATION_LFO as usize) as f32
        )
    }

    pub(crate) fn get_delay_vibrato_lfo(&self) -> f32 {
        SoundFontMath::timecents_to_seconds(
            self.gs(GeneratorType::DELAY_VIBRATO_LFO as usize) as f32
        )
    }

    pub(crate) fn get_frequency_vibrato_lfo(&self) -> f32 {
        SoundFontMath::cents_to_hertz(self.gs(GeneratorType::FREQUENCY_VIBRATO_LFO as usize) as f32)
    }

    pub(crate) fn get_delay_modulation_envelope(&self) -> f32 {
        SoundFontMath::timecents_to_seconds(
            self.gs(GeneratorType::DELAY_MODULATION_ENVELOPE as usize) as f32,
        )
    }

    pub(crate) fn get_attack_modulation_envelope(&self) -> f32 {
        SoundFontMath::timecents_to_seconds(
            self.gs(GeneratorType::ATTACK_MODULATION_ENVELOPE as usize) as f32,
        )
    }

    pub(crate) fn get_hold_modulation_envelope(&self) -> f32 {
        SoundFontMath::timecents_to_seconds(
            self.gs(GeneratorType::HOLD_MODULATION_ENVELOPE as usize) as f32,
        )
    }

    pub(crate) fn get_decay_modulation_envelope(&self) -> f32 {
        SoundFontMath::timecents_to_seconds(
            self.gs(GeneratorType::DECAY_MODULATION_ENVELOPE as usize) as f32,
        )
    }

    pub(crate) fn get_sustain_modulation_envelope(&self) -> f32 {
        0.1_f32 * self.gs(GeneratorType::SUSTAIN_MODULATION_ENVELOPE as usize) as f32
    }

    pub(crate) fn get_release_modulation_envelope(&self) -> f32 {
        SoundFontMath::timecents_to_seconds(
            self.gs(GeneratorType::RELEASE_MODULATION_ENVELOPE as usize) as f32,
        )
    }

    pub(crate) fn get_key_number_to_modulation_envelope_hold(&self) -> i32 {
        self.gs(GeneratorType::KEY_NUMBER_TO_MODULATION_ENVELOPE_HOLD as usize)
    }

    pub(crate) fn get_key_number_to_modulation_envelope_decay(&self) -> i32 {
        self.gs(GeneratorType::KEY_NUMBER_TO_MODULATION_ENVELOPE_DECAY as usize)
    }

    pub(crate) fn get_delay_volume_envelope(&self) -> f32 {
        SoundFontMath::timecents_to_seconds(
            self.gs(GeneratorType::DELAY_VOLUME_ENVELOPE as usize) as f32
        )
    }

    pub(crate) fn get_attack_volume_envelope(&self) -> f32 {
        SoundFontMath::timecents_to_seconds(
            self.gs(GeneratorType::ATTACK_VOLUME_ENVELOPE as usize) as f32
        )
    }

    pub(crate) fn get_hold_volume_envelope(&self) -> f32 {
        SoundFontMath::timecents_to_seconds(
            self.gs(GeneratorType::HOLD_VOLUME_ENVELOPE as usize) as f32
        )
    }

    pub(crate) fn get_decay_volume_envelope(&self) -> f32 {
        SoundFontMath::timecents_to_seconds(
            self.gs(GeneratorType::DECAY_VOLUME_ENVELOPE as usize) as f32
        )
    }

    pub(crate) fn get_sustain_volume_envelope(&self) -> f32 {
        0.1_f32 * self.gs(GeneratorType::SUSTAIN_VOLUME_ENVELOPE as usize) as f32
    }

    pub(crate) fn get_release_volume_envelope(&self) -> f32 {
        SoundFontMath::timecents_to_seconds(
            self.gs(GeneratorType::RELEASE_VOLUME_ENVELOPE as usize) as f32,
        )
    }

    pub(crate) fn get_key_number_to_volume_envelope_hold(&self) -> i32 {
        self.gs(GeneratorType::KEY_NUMBER_TO_VOLUME_ENVELOPE_HOLD as usize)
    }

    pub(crate) fn get_key_number_to_volume_envelope_decay(&self) -> i32 {
        self.gs(GeneratorType::KEY_NUMBER_TO_VOLUME_ENVELOPE_DECAY as usize)
    }

    pub(crate) fn get_initial_attenuation(&self) -> f32 {
        0.1_f32 * self.gs(GeneratorType::INITIAL_ATTENUATION as usize) as f32
    }

    pub(crate) fn get_coarse_tune(&self) -> i32 {
        self.gs(GeneratorType::COARSE_TUNE as usize)
    }

    pub(crate) fn get_fine_tune(&self) -> i32 {
        self.gs(GeneratorType::FINE_TUNE as usize) + self.instrument.sample_pitch_correction
    }

    pub(crate) fn get_sample_modes(&self) -> LoopMode {
        self.instrument.get_sample_modes()
    }

    pub(crate) fn get_scale_tuning(&self) -> i32 {
        self.gs(GeneratorType::SCALE_TUNING as usize)
    }

    pub(crate) fn get_exclusive_class(&self) -> i32 {
        self.instrument.get_exclusive_class()
    }

    pub(crate) fn get_root_key(&self) -> i32 {
        self.instrument.get_root_key()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_ranges_cover_the_restricted_generators() {
        assert_eq!(generator_range(GeneratorType::INITIAL_ATTENUATION as usize), Some((0, 1440)));
        assert_eq!(generator_range(GeneratorType::PAN as usize), Some((-500, 500)));
        // Generators this file does not restrict pass through untouched.
        assert_eq!(generator_range(GeneratorType::SAMPLE_MODES as usize), None);
    }
}
