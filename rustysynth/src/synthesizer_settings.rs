#![allow(dead_code)]

use crate::error::SynthesizerError;
use crate::volume_attack_curve::VolumeAttackCurve;

/// Specifies a set of parameters for synthesis.
#[derive(Debug)]
#[non_exhaustive]
pub struct SynthesizerSettings {
    /// The sample rate for synthesis.
    pub sample_rate: i32,
    /// The block size for rendering waveform.
    pub block_size: usize,
    /// The number of maximum polyphony.
    pub maximum_polyphony: usize,
    /// The value indicating whether reverb and chorus are enabled.
    pub enable_reverb_and_chorus: bool,
    /// The shape of the volume envelope attack stage.
    pub volume_attack_curve: VolumeAttackCurve,
    /// The value indicating whether the SF2 default modulator from note-on velocity to
    /// filter cutoff is applied. It lowers the cutoff by up to two octaves for soft notes,
    /// even for instruments that do not otherwise use the filter. A SoundFont modulator with
    /// the same definition still overrides this setting.
    pub enable_velocity_to_filter_cutoff: bool,
    /// The value indicating whether modulators defined in the SoundFont are applied.
    /// When disabled, only the SF2 default modulators are used.
    pub enable_soundfont_modulators: bool,
}

impl SynthesizerSettings {
    const DEFAULT_BLOCK_SIZE: usize = 64;
    const DEFAULT_MAXIMUM_POLYPHONY: usize = 64;
    const DEFAULT_ENABLE_REVERB_AND_CHORUS: bool = true;
    const DEFAULT_VOLUME_ATTACK_CURVE: VolumeAttackCurve = VolumeAttackCurve::Cubic;
    const DEFAULT_ENABLE_VELOCITY_TO_FILTER_CUTOFF: bool = true;
    const DEFAULT_ENABLE_SOUNDFONT_MODULATORS: bool = true;

    /// Initializes a new instance of synthesizer settings.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - The sample rate for synthesis.
    pub fn new(sample_rate: i32) -> Self {
        Self {
            sample_rate,
            block_size: SynthesizerSettings::DEFAULT_BLOCK_SIZE,
            maximum_polyphony: SynthesizerSettings::DEFAULT_MAXIMUM_POLYPHONY,
            enable_reverb_and_chorus: SynthesizerSettings::DEFAULT_ENABLE_REVERB_AND_CHORUS,
            volume_attack_curve: SynthesizerSettings::DEFAULT_VOLUME_ATTACK_CURVE,
            enable_velocity_to_filter_cutoff:
                SynthesizerSettings::DEFAULT_ENABLE_VELOCITY_TO_FILTER_CUTOFF,
            enable_soundfont_modulators: SynthesizerSettings::DEFAULT_ENABLE_SOUNDFONT_MODULATORS,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), SynthesizerError> {
        SynthesizerSettings::check_sample_rate(self.sample_rate)?;
        SynthesizerSettings::check_block_size(self.block_size)?;
        SynthesizerSettings::check_maximum_polyphony(self.maximum_polyphony)?;

        Ok(())
    }

    fn check_sample_rate(value: i32) -> Result<(), SynthesizerError> {
        if !(16_000..=192_000).contains(&value) {
            return Err(SynthesizerError::SampleRateOutOfRange(value));
        }

        Ok(())
    }

    fn check_block_size(value: usize) -> Result<(), SynthesizerError> {
        if !(8..=1024).contains(&value) {
            return Err(SynthesizerError::BlockSizeOutOfRange(value));
        }

        Ok(())
    }

    fn check_maximum_polyphony(value: usize) -> Result<(), SynthesizerError> {
        if !(8..=256).contains(&value) {
            return Err(SynthesizerError::MaximumPolyphonyOutOfRange(value));
        }

        Ok(())
    }
}
