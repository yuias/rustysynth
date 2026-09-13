#![allow(dead_code)]

use crate::channel::Channel;
use crate::generator_type::GeneratorType;
use crate::modulator::Modulator;
use crate::modulator_source::{Controller, ModulatorSource};

/// The SF2.04 default modulators (section 8.4), in the order of `DefaultModulator`.
///
/// The voice renders these through dedicated code paths. A SoundFont modulator with the
/// same identity only changes the amount, which is applied to those paths as a scale.
pub(crate) const DEFAULT_MODULATORS: [Modulator; DEFAULT_MODULATOR_COUNT] = [
    default_modulator(0x0502, GeneratorType::INITIAL_ATTENUATION, 960, 0),
    default_modulator(0x0102, GeneratorType::INITIAL_FILTER_CUTOFF_FREQUENCY, -2400, 0x0D02),
    default_modulator(0x000D, GeneratorType::VIBRATO_LFO_TO_PITCH, 50, 0),
    default_modulator(0x0081, GeneratorType::VIBRATO_LFO_TO_PITCH, 50, 0),
    default_modulator(0x0587, GeneratorType::INITIAL_ATTENUATION, 960, 0),
    default_modulator(0x028A, GeneratorType::PAN, 1000, 0),
    default_modulator(0x058B, GeneratorType::INITIAL_ATTENUATION, 960, 0),
    default_modulator(0x00DB, GeneratorType::REVERB_EFFECTS_SEND, 200, 0),
    default_modulator(0x00DD, GeneratorType::CHORUS_EFFECTS_SEND, 200, 0),
    default_modulator(0x020E, GeneratorType::FINE_TUNE, 12700, 0x0010),
];

pub(crate) const DEFAULT_MODULATOR_COUNT: usize = 10;

/// Upper bound of non-default modulators per voice; the rest are ignored.
pub(crate) const MAX_VOICE_MODULATORS: usize = 32;

/// Generator offsets produced by modulators, in generator units.
pub(crate) type GeneratorOffsets = [f32; GeneratorType::COUNT];

/// Whether the voice applies a destination on every block. Other destinations (envelope
/// and LFO timing, key scaling) are only read when the note starts.
pub(crate) fn is_realtime_destination(destination: u16) -> bool {
    matches!(
        destination,
        GeneratorType::MODULATION_LFO_TO_PITCH
            | GeneratorType::VIBRATO_LFO_TO_PITCH
            | GeneratorType::MODULATION_ENVELOPE_TO_PITCH
            | GeneratorType::INITIAL_FILTER_CUTOFF_FREQUENCY
            | GeneratorType::INITIAL_FILTER_Q
            | GeneratorType::MODULATION_LFO_TO_FILTER_CUTOFF_FREQUENCY
            | GeneratorType::MODULATION_ENVELOPE_TO_FILTER_CUTOFF_FREQUENCY
            | GeneratorType::MODULATION_LFO_TO_VOLUME
            | GeneratorType::CHORUS_EFFECTS_SEND
            | GeneratorType::REVERB_EFFECTS_SEND
            | GeneratorType::PAN
            | GeneratorType::INITIAL_ATTENUATION
            | GeneratorType::COARSE_TUNE
            | GeneratorType::FINE_TUNE
    )
}

const fn default_modulator(source: u16, destination: u16, amount: i16, amount_source: u16) -> Modulator {
    Modulator {
        source,
        destination,
        amount,
        amount_source,
        transform: 0,
    }
}

/// Index into `DEFAULT_MODULATORS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DefaultModulator {
    VelocityToAttenuation = 0,
    VelocityToFilterCutoff = 1,
    ChannelPressureToVibrato = 2,
    ModulationWheelToVibrato = 3,
    VolumeToAttenuation = 4,
    PanToPan = 5,
    ExpressionToAttenuation = 6,
    ReverbSend = 7,
    ChorusSend = 8,
    PitchWheelToFineTune = 9,
}

/// The modulators that apply to one voice, merged at note-on without allocating.
#[derive(Debug)]
pub(crate) struct VoiceModulators {
    /// SoundFont modulators that do not override a default modulator.
    items: [Modulator; MAX_VOICE_MODULATORS],
    item_amounts: [f32; MAX_VOICE_MODULATORS],
    len: usize,
    realtime_len: usize,
    /// Effective amount of each default modulator divided by its SF2 default amount.
    default_scales: [f32; DEFAULT_MODULATOR_COUNT],
}

impl VoiceModulators {
    pub(crate) fn new() -> Self {
        Self {
            items: [DEFAULT_MODULATORS[0]; MAX_VOICE_MODULATORS],
            item_amounts: [0_f32; MAX_VOICE_MODULATORS],
            len: 0,
            realtime_len: 0,
            default_scales: [1_f32; DEFAULT_MODULATOR_COUNT],
        }
    }

    /// Merges the default, instrument and preset modulators for a new note.
    ///
    /// Per SF2.04 9.5, an instrument modulator replaces an identical default modulator,
    /// and a preset modulator adds its amount to the identical instrument or default one.
    pub(crate) fn start(
        &mut self,
        instrument: &[Modulator],
        preset: &[Modulator],
        enable_soundfont_modulators: bool,
        enable_velocity_to_filter_cutoff: bool,
    ) {
        let mut amounts = [0_f32; DEFAULT_MODULATOR_COUNT];
        for (amount, default) in amounts.iter_mut().zip(DEFAULT_MODULATORS.iter()) {
            *amount = default.amount as f32;
        }
        if !enable_velocity_to_filter_cutoff {
            amounts[DefaultModulator::VelocityToFilterCutoff as usize] = 0_f32;
        }

        self.len = 0;

        if enable_soundfont_modulators {
            for modulator in instrument {
                match Self::find_default(modulator) {
                    Some(k) => amounts[k] = modulator.amount as f32,
                    None => self.push(modulator),
                }
            }

            for modulator in preset {
                if let Some(k) = Self::find_default(modulator) {
                    amounts[k] += modulator.amount as f32;
                } else if let Some(i) = self.items[..self.len]
                    .iter()
                    .position(|item| item.same_identity(modulator))
                {
                    self.item_amounts[i] += modulator.amount as f32;
                } else {
                    self.push(modulator);
                }
            }
        }

        for (k, scale) in self.default_scales.iter_mut().enumerate() {
            *scale = amounts[k] / DEFAULT_MODULATORS[k].amount as f32;
        }

        self.realtime_len = self.items[..self.len]
            .iter()
            .filter(|item| is_realtime_destination(item.destination))
            .count();
    }

    /// True when some modulator targets a destination that is applied on every block.
    pub(crate) fn has_realtime_items(&self) -> bool {
        self.realtime_len > 0
    }

    /// Overwrites `offsets` with the sum of the modulators whose destinations are realtime
    /// (`realtime == true`) or note-on only (`realtime == false`).
    pub(crate) fn evaluate(
        &self,
        channel: &Channel,
        key: i32,
        velocity: i32,
        realtime: bool,
        offsets: &mut GeneratorOffsets,
    ) {
        offsets.fill(0_f32);
        for (item, &amount) in self.items[..self.len].iter().zip(self.item_amounts.iter()) {
            if is_realtime_destination(item.destination) != realtime {
                continue;
            }
            let source = source_value(ModulatorSource(item.source), channel, key, velocity);
            let amount_source = ModulatorSource(item.amount_source);
            let scale = match amount_source.controller() {
                Some(Controller::None) => 1_f32,
                _ => source_value(amount_source, channel, key, velocity),
            };
            let mut value = amount * source * scale;
            // Transform 2 is absolute value; other transforms were rejected at load.
            if item.transform == 2 {
                value = value.abs();
            }
            offsets[item.destination as usize] += value;
        }
    }

    fn find_default(modulator: &Modulator) -> Option<usize> {
        DEFAULT_MODULATORS
            .iter()
            .position(|default| default.same_identity(modulator))
    }

    fn push(&mut self, modulator: &Modulator) {
        if self.len < MAX_VOICE_MODULATORS {
            self.items[self.len] = *modulator;
            self.item_amounts[self.len] = modulator.amount as f32;
            self.len += 1;
        }
    }

    /// Scale to apply to the dedicated code path of a default modulator (1.0 = SF2 default).
    pub(crate) fn default_scale(&self, modulator: DefaultModulator) -> f32 {
        self.default_scales[modulator as usize]
    }

    /// Non-default modulators and their merged amounts.
    pub(crate) fn items(&self) -> impl Iterator<Item = (&Modulator, f32)> {
        self.items[..self.len]
            .iter()
            .zip(self.item_amounts[..self.len].iter().copied())
    }
}

fn source_value(source: ModulatorSource, channel: &Channel, key: i32, velocity: i32) -> f32 {
    let clamp_7bit = |value: i32| value.clamp(0, 127) as u8;
    match source.controller() {
        Some(Controller::Velocity) => source.map_7bit(clamp_7bit(velocity)),
        Some(Controller::Key) => source.map_7bit(clamp_7bit(key)),
        Some(Controller::PolyPressure) => source.map_7bit(channel.get_poly_pressure(key)),
        Some(Controller::ChannelPressure) => source.map_7bit(channel.get_channel_pressure_raw()),
        Some(Controller::PitchWheel) => source.map_14bit(channel.get_pitch_bend_raw()),
        Some(Controller::PitchWheelSensitivity) => {
            source.map_7bit(clamp_7bit(channel.get_pitch_bend_range() as i32))
        }
        Some(Controller::Cc(controller)) => source.map_7bit(channel.get_controller_value(controller)),
        Some(Controller::None) | None => 0_f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modulator(source: u16, destination: u16, amount: i16, amount_source: u16) -> Modulator {
        default_modulator(source, destination, amount, amount_source)
    }

    #[test]
    fn no_modulators_keeps_default_scales() {
        let mut modulators = VoiceModulators::new();
        modulators.start(&[], &[], true, true);
        for k in 0..DEFAULT_MODULATOR_COUNT {
            assert_eq!(modulators.default_scales[k], 1.0);
        }
        assert_eq!(modulators.items().count(), 0);
    }

    #[test]
    fn instrument_replaces_and_preset_adds_to_default() {
        let mut modulators = VoiceModulators::new();
        let instrument = [modulator(0x0102, GeneratorType::INITIAL_FILTER_CUTOFF_FREQUENCY, 0, 0x0D02)];
        let preset = [
            modulator(0x0502, GeneratorType::INITIAL_ATTENUATION, 960, 0),
            modulator(0x00DB, GeneratorType::REVERB_EFFECTS_SEND, 800, 0),
        ];
        modulators.start(&instrument, &preset, true, true);

        assert_eq!(modulators.default_scale(DefaultModulator::VelocityToFilterCutoff), 0.0);
        assert_eq!(modulators.default_scale(DefaultModulator::VelocityToAttenuation), 2.0);
        assert_eq!(modulators.default_scale(DefaultModulator::ReverbSend), 5.0);
        assert_eq!(modulators.items().count(), 0);
    }

    #[test]
    fn non_default_modulators_are_collected_and_preset_amounts_added() {
        let mut modulators = VoiceModulators::new();
        // CC2 -> attenuation, and the same without the concave curve (different identity).
        let instrument = [
            modulator(0x0582, GeneratorType::INITIAL_ATTENUATION, 480, 0),
            modulator(0x0182, GeneratorType::INITIAL_ATTENUATION, 100, 0),
        ];
        let preset = [modulator(0x0582, GeneratorType::INITIAL_ATTENUATION, 120, 0)];
        modulators.start(&instrument, &preset, true, true);

        let items: Vec<_> = modulators.items().map(|(m, amount)| (m.source, amount)).collect();
        assert_eq!(items, vec![(0x0582, 600.0), (0x0182, 100.0)]);
    }

    #[test]
    fn settings_disable_file_modulators_and_velocity_to_cutoff() {
        let mut modulators = VoiceModulators::new();
        let instrument = [modulator(0x0081, GeneratorType::VIBRATO_LFO_TO_PITCH, 0, 0)];
        modulators.start(&instrument, &[], false, false);

        assert_eq!(modulators.default_scale(DefaultModulator::ModulationWheelToVibrato), 1.0);
        assert_eq!(modulators.default_scale(DefaultModulator::VelocityToFilterCutoff), 0.0);

        // A file modulator can still restore the default the setting turned off.
        let instrument = [modulator(0x0102, GeneratorType::INITIAL_FILTER_CUTOFF_FREQUENCY, -1200, 0x0D02)];
        modulators.start(&instrument, &[], true, false);
        assert_eq!(modulators.default_scale(DefaultModulator::VelocityToFilterCutoff), 0.5);
    }

    #[test]
    fn modulators_beyond_capacity_are_ignored() {
        let mut modulators = VoiceModulators::new();
        let instrument: Vec<Modulator> = (0..40)
            .map(|i| modulator(0x0080 | (i + 1), GeneratorType::PAN, 10, 0))
            .collect();
        modulators.start(&instrument, &[], true, true);
        assert_eq!(modulators.items().count(), MAX_VOICE_MODULATORS);
    }
}
