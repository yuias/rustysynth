#![allow(dead_code)]

use std::f32::consts;

use crate::bi_quad_filter::BiQuadFilter;
use crate::channel::Channel;
use crate::lfo::Lfo;
use crate::master_tune::MasterTune;
use crate::modulation_envelope::ModulationEnvelope;
use crate::oscillator::Oscillator;
use crate::region_ex::RegionEx;
use crate::region_pair::RegionPair;
use crate::soundfont_math::SoundFontMath;
use crate::synthesizer_settings::SynthesizerSettings;
use crate::generator_type::GeneratorType;
use crate::voice_modulators::{DefaultModulator, GeneratorOffsets, VoiceModulators};
use crate::volume_envelope::VolumeEnvelope;

#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
enum VoiceState {
    Playing = 0,
    ReleaseRequested = 1,
    Released = 2,
}

#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct Voice {
    vol_env: VolumeEnvelope,
    mod_env: ModulationEnvelope,

    vib_lfo: Lfo,
    mod_lfo: Lfo,

    oscillator: Oscillator,
    filter: BiQuadFilter,

    block: Vec<f32>,

    // A sudden change in the mix gain will cause pop noise.
    // To avoid this, we save the mix gain of the previous block,
    // and smooth out the gain if the gap between the current and previous gain is too large.
    // The actual smoothing process is done in the WriteBlock method of the Synthesizer class.
    pub(crate) previous_mix_gain_left: f32,
    pub(crate) previous_mix_gain_right: f32,
    pub(crate) current_mix_gain_left: f32,
    pub(crate) current_mix_gain_right: f32,

    pub(crate) previous_reverb_send: f32,
    pub(crate) previous_chorus_send: f32,
    pub(crate) current_reverb_send: f32,
    pub(crate) current_chorus_send: f32,

    exclusive_class: i32,
    channel: i32,
    key: i32,
    velocity: i32,

    note_gain: f32,

    cutoff: f32,
    resonance: f32,

    vib_lfo_to_pitch: f32,
    mod_lfo_to_pitch: f32,
    mod_env_to_pitch: f32,

    mod_lfo_to_cutoff: i32,
    mod_env_to_cutoff: i32,
    dynamic_cutoff: bool,

    mod_lfo_to_volume: f32,
    dynamic_volume: bool,

    instrument_pan: f32,
    // Pitch offset in semitones from the GS drum instrument NRPN, fixed for the note.
    drum_pitch_coarse: f32,
    instrument_reverb: f32,
    instrument_chorus: f32,

    // Some instruments require fast cutoff change, which can cause pop noise.
    // This is used to smooth out the cutoff frequency.
    smoothed_cutoff: f32,
    filter_q_scale: f32,

    // Portamento: pitch offset that decays towards 0
    portamento_offset: f32,    // current pitch offset in semitones
    portamento_speed: f32,     // semitones per sample (0 = no portamento)

    voice_state: VoiceState,
    /// Time elapsed in samples
    voice_length: usize,
    min_voice_length: usize,
    // Set when the sostenuto pedal went down while this note's key was held.
    sostenuto_captured: bool,
    enable_velocity_to_filter_cutoff: bool,
    enable_soundfont_modulators: bool,
    modulators: VoiceModulators,
    // Offsets of destinations applied per block, re-evaluated while controllers can change.
    realtime_offsets: GeneratorOffsets,
}

impl Voice {
    pub(crate) fn new(settings: &SynthesizerSettings) -> Self {
        Self {
            vol_env: VolumeEnvelope::new(settings),
            mod_env: ModulationEnvelope::new(settings),
            vib_lfo: Lfo::new(settings),
            mod_lfo: Lfo::new(settings),
            oscillator: Oscillator::new(settings),
            filter: BiQuadFilter::new(settings),
            block: vec![0_f32; settings.block_size],
            previous_mix_gain_left: 0_f32,
            previous_mix_gain_right: 0_f32,
            current_mix_gain_left: 0_f32,
            current_mix_gain_right: 0_f32,
            previous_reverb_send: 0_f32,
            previous_chorus_send: 0_f32,
            current_reverb_send: 0_f32,
            current_chorus_send: 0_f32,
            exclusive_class: 0,
            channel: 0,
            key: 0,
            velocity: 0,
            note_gain: 0_f32,
            cutoff: 0_f32,
            resonance: 0_f32,
            vib_lfo_to_pitch: 0_f32,
            mod_lfo_to_pitch: 0_f32,
            mod_env_to_pitch: 0_f32,
            mod_lfo_to_cutoff: 0,
            mod_env_to_cutoff: 0,
            dynamic_cutoff: false,
            mod_lfo_to_volume: 0_f32,
            dynamic_volume: false,
            instrument_pan: 0_f32,
            drum_pitch_coarse: 0_f32,
            instrument_reverb: 0_f32,
            instrument_chorus: 0_f32,
            smoothed_cutoff: 0_f32,
            filter_q_scale: 1_f32,
            portamento_offset: 0_f32,
            portamento_speed: 0_f32,
            voice_state: VoiceState::Playing,
            voice_length: 0,
            min_voice_length: (settings.sample_rate / 500) as usize,
            sostenuto_captured: false,
            enable_velocity_to_filter_cutoff: settings.enable_velocity_to_filter_cutoff,
            enable_soundfont_modulators: settings.enable_soundfont_modulators,
            modulators: VoiceModulators::new(),
            realtime_offsets: [0_f32; GeneratorType::COUNT],
        }
    }

    pub(crate) fn start(&mut self, region: &RegionPair, channel_info: &Channel, channel: i32, key: i32, velocity: i32,
        portamento_source: i32, portamento_speed: f32) {
        self.exclusive_class = region.get_exclusive_class();
        self.modulators.start(
            &region.instrument.modulators,
            &region.preset.modulators,
            self.enable_soundfont_modulators,
            self.enable_velocity_to_filter_cutoff,
        );
        // Destinations read only at note-on take the modulator offsets through the region;
        // realtime destinations are applied on top of the region values in process().
        let mut note_on_offsets: GeneratorOffsets = [0_f32; GeneratorType::COUNT];
        self.modulators
            .evaluate(channel_info, key, velocity, false, &mut note_on_offsets);
        self.modulators
            .evaluate(channel_info, key, velocity, true, &mut self.realtime_offsets);
        let region = &region.with_offsets(&note_on_offsets);
        self.channel = channel;
        self.key = key;
        self.velocity = velocity;

        if velocity > 0 {
            // According to the Polyphone's implementation, the initial attenuation should be reduced to 40%.
            // I'm not sure why, but this indeed improves the loudness variability.
            let sample_attenuation = 0.4_f32 * region.get_initial_attenuation();
            let filter_attenuation = 0.5_f32 * region.get_initial_filter_q();
            let decibels = 2_f32 * SoundFontMath::linear_to_decibels(velocity as f32 / 127_f32)
                * self.modulators.default_scale(DefaultModulator::VelocityToAttenuation)
                - sample_attenuation
                - filter_attenuation;
            self.note_gain = SoundFontMath::decibels_to_linear(decibels);
            // The GS drum instrument level scales the note rather than replacing the
            // SoundFont attenuation, which the region still sets per instrument.
            if let Some(level) = channel_info.get_drum_level(key) {
                self.note_gain *= level;
            }
        } else {
            self.note_gain = 0_f32;
        }

        self.cutoff = region.get_initial_filter_cutoff_frequency();
        // SF2 Default Modulator #2: Note-On Velocity → Filter Cutoff
        // Source: velocity, linear, unipolar, negative. Amount: -2400 cents.
        // At vel=0: cutoff reduced by 2400 cents (2 octaves). At vel=127: no change.
        let vel_fc_amount = -2400.0
            * self.modulators.default_scale(DefaultModulator::VelocityToFilterCutoff);
        if vel_fc_amount != 0.0 && velocity < 127 {
            let vel_fc_cents = vel_fc_amount * (1.0 - velocity as f32 / 127.0);
            self.cutoff *= SoundFontMath::cents_to_multiplying_factor(vel_fc_cents);
        }
        self.resonance = SoundFontMath::decibels_to_linear(region.get_initial_filter_q());

        self.vib_lfo_to_pitch = 0.01_f32 * region.get_vibrato_lfo_to_pitch() as f32
            * channel_info.get_vibrato_depth_multiplier();
        self.mod_lfo_to_pitch = 0.01_f32 * region.get_modulation_lfo_to_pitch() as f32;
        self.mod_env_to_pitch = 0.01_f32 * region.get_modulation_envelope_to_pitch() as f32;

        self.mod_lfo_to_cutoff = region.get_modulation_lfo_to_filter_cutoff_frequency();
        self.mod_env_to_cutoff = region.get_modulation_envelope_to_filter_cutoff_frequency();
        self.dynamic_cutoff = self.mod_lfo_to_cutoff != 0 || self.mod_env_to_cutoff != 0;

        self.mod_lfo_to_volume = region.get_modulation_lfo_to_volume();
        self.dynamic_volume = self.mod_lfo_to_volume > 0.05_f32;

        self.instrument_pan = SoundFontMath::clamp(region.get_pan(), -50_f32, 50_f32);
        self.instrument_reverb = 0.01_f32 * region.get_reverb_effects_send();
        self.instrument_chorus = 0.01_f32 * region.get_chorus_effects_send();

        // The GS drum instrument parameters address one note of the kit, so they replace the
        // region's own pan and sends rather than adding to them.
        if let Some(pan) = channel_info.get_drum_pan(key) {
            self.instrument_pan = pan;
        }
        if let Some(send) = channel_info.get_drum_reverb_send(key) {
            self.instrument_reverb = send;
        }
        if let Some(send) = channel_info.get_drum_chorus_send(key) {
            self.instrument_chorus = send;
        }
        self.drum_pitch_coarse = channel_info.get_drum_pitch_coarse(key);

        RegionEx::start_volume_envelope(&mut self.vol_env, region, channel_info, key, velocity);
        RegionEx::start_modulation_envelope(&mut self.mod_env, region, key, velocity);
        RegionEx::start_vibrato(&mut self.vib_lfo, region, channel_info, key, velocity);
        RegionEx::start_modulation(&mut self.mod_lfo, region, key, velocity);
        RegionEx::start_oscillator(&mut self.oscillator, region);
        self.filter.clear_buffer();
        self.filter.set_low_pass_filter(self.cutoff, self.resonance, 1_f32);

        self.smoothed_cutoff = self.cutoff;
        self.filter_q_scale = 1_f32;

        // Portamento: set initial pitch offset from source to target key
        if portamento_speed > 0.0 && portamento_source >= 0 && portamento_source != key {
            self.portamento_offset = (portamento_source - key) as f32;
            self.portamento_speed = portamento_speed;
        } else {
            self.portamento_offset = 0.0;
            self.portamento_speed = 0.0;
        }

        self.voice_state = VoiceState::Playing;
        self.voice_length = 0;
        self.sostenuto_captured = false;
    }

    pub(crate) fn end(&mut self) {
        if self.voice_state == VoiceState::Playing {
            self.voice_state = VoiceState::ReleaseRequested;
        }
    }

    pub(crate) fn kill(&mut self) {
        self.note_gain = 0_f32;
    }

    pub(crate) fn process(
        &mut self,
        data: &[i16],
        channels: &[Channel],
        master_tune: MasterTune,
    ) -> bool {
        if self.note_gain < SoundFontMath::NON_AUDIBLE {
            return false;
        }

        let channel_info = &channels[self.channel as usize];

        self.release_if_necessary(channel_info);

        if !self.vol_env.process(self.block.len()) {
            return false;
        }

        self.mod_env.process(self.block.len());
        self.vib_lfo.process();
        self.mod_lfo.process();

        if self.modulators.has_realtime_items() {
            self.modulators.evaluate(
                channel_info,
                self.key,
                self.velocity,
                true,
                &mut self.realtime_offsets,
            );
        }
        let offset = |destination: u16| self.realtime_offsets[destination as usize];
        let cutoff_offset = offset(GeneratorType::INITIAL_FILTER_CUTOFF_FREQUENCY);
        let q_offset = offset(GeneratorType::INITIAL_FILTER_Q);
        let lfo_to_cutoff_offset = offset(GeneratorType::MODULATION_LFO_TO_FILTER_CUTOFF_FREQUENCY);
        let env_to_cutoff_offset =
            offset(GeneratorType::MODULATION_ENVELOPE_TO_FILTER_CUTOFF_FREQUENCY);
        let lfo_to_volume_offset = offset(GeneratorType::MODULATION_LFO_TO_VOLUME);
        let attenuation_offset = offset(GeneratorType::INITIAL_ATTENUATION);
        let pan_offset = offset(GeneratorType::PAN);
        let reverb_offset = offset(GeneratorType::REVERB_EFFECTS_SEND);
        let chorus_offset = offset(GeneratorType::CHORUS_EFFECTS_SEND);

        // SF2 Default Modulator #10: Channel Pressure → Vibrato LFO Pitch Depth
        // Source: channel pressure, linear, unipolar, positive. Amount: 50 cents.
        let pressure_vib = 0.01_f32 * 50.0 * channel_info.get_channel_pressure()
            * self.modulators.default_scale(DefaultModulator::ChannelPressureToVibrato);
        let wheel_vib = 0.01_f32 * channel_info.get_modulation()
            * self.modulators.default_scale(DefaultModulator::ModulationWheelToVibrato);
        let vib_lfo_to_pitch =
            self.vib_lfo_to_pitch + 0.01_f32 * offset(GeneratorType::VIBRATO_LFO_TO_PITCH);
        let mod_lfo_to_pitch =
            self.mod_lfo_to_pitch + 0.01_f32 * offset(GeneratorType::MODULATION_LFO_TO_PITCH);
        let mod_env_to_pitch =
            self.mod_env_to_pitch + 0.01_f32 * offset(GeneratorType::MODULATION_ENVELOPE_TO_PITCH);
        let modulator_tune = offset(GeneratorType::COARSE_TUNE)
            + 0.01_f32 * offset(GeneratorType::FINE_TUNE);

        let vib_depth = wheel_vib + vib_lfo_to_pitch + pressure_vib;
        let mod_env_pitch = mod_env_to_pitch * self.mod_env.get_value();
        let channel_pitch_change = channel_info.get_tune()
            + channel_info.get_pitch_bend()
                * self.modulators.default_scale(DefaultModulator::PitchWheelToFineTune);
        let scale_tuning = channel_info.get_scale_tuning_for_key(self.key);
        let master_tune = master_tune.for_channel(channel_info.get_is_percussion_channel());
        let base_pitch = self.key as f32 + mod_env_pitch + channel_pitch_change + master_tune
            + scale_tuning + modulator_tune + self.drum_pitch_coarse;

        let (portamento_start, portamento_end) = self.advance_portamento();

        // Compute pitch at block boundaries for per-sample LFO interpolation
        let pitch_start = base_pitch
            + portamento_start
            + vib_depth * self.vib_lfo.get_prev_value()
            + mod_lfo_to_pitch * self.mod_lfo.get_prev_value();
        let pitch_end = base_pitch
            + portamento_end
            + vib_depth * self.vib_lfo.get_value()
            + mod_lfo_to_pitch * self.mod_lfo.get_value();
        if !self.oscillator.process(data, &mut self.block[..], pitch_start, pitch_end) {
            return false;
        }

        {
            // Apply CC#74 (Brightness) as cutoff offset in cents
            let brightness_cents = channel_info.get_brightness_cents();
            // Apply CC#71 (Resonance) as resonance offset in dB
            let resonance_db = channel_info.get_filter_resonance_db();

            let q_scale = if resonance_db != 0.0 {
                SoundFontMath::decibels_to_linear(resonance_db)
            } else {
                1_f32
            };

            // Keep updating until the filter has settled back after a controller returns
            // to neutral; otherwise the last offset stays applied for the rest of the note.
            let needs_update = self.dynamic_cutoff
                || brightness_cents != 0.0
                || q_scale != self.filter_q_scale
                || self.smoothed_cutoff != self.cutoff
                || self.modulators.has_realtime_items();

            if needs_update {
                let mod_cents = (self.mod_lfo_to_cutoff as f32 + lfo_to_cutoff_offset)
                    * self.mod_lfo.get_value()
                    + (self.mod_env_to_cutoff as f32 + env_to_cutoff_offset)
                        * self.mod_env.get_value();
                let total_cents = mod_cents + brightness_cents + cutoff_offset;
                let factor = SoundFontMath::cents_to_multiplying_factor(total_cents);
                let new_cutoff = factor * self.cutoff;

                // The cutoff change is limited within x0.5 and x2 to reduce pop noise.
                let lower_limit = 0.5_f32 * self.smoothed_cutoff;
                let upper_limit = 2_f32 * self.smoothed_cutoff;
                self.smoothed_cutoff = SoundFontMath::clamp(new_cutoff, lower_limit, upper_limit);

                let resonance = if q_offset != 0.0 {
                    self.resonance * SoundFontMath::decibels_to_linear(0.1_f32 * q_offset)
                } else {
                    self.resonance
                };
                self.filter
                    .set_low_pass_filter(self.smoothed_cutoff, resonance, q_scale);
                self.filter_q_scale = q_scale;
            }
        }
        self.filter.process(&mut self.block[..]);

        self.previous_mix_gain_left = self.current_mix_gain_left;
        self.previous_mix_gain_right = self.current_mix_gain_right;
        self.previous_reverb_send = self.current_reverb_send;
        self.previous_chorus_send = self.current_chorus_send;

        // According to the GM spec, the following value should be squared.
        // The square is the SF2 default CC7/CC11 modulators; their scales change the exponent.
        let volume_scale = self.modulators.default_scale(DefaultModulator::VolumeToAttenuation);
        let expression_scale =
            self.modulators.default_scale(DefaultModulator::ExpressionToAttenuation);
        let channel_gain = if volume_scale == 1_f32 && expression_scale == 1_f32 {
            let ve = channel_info.get_volume() * channel_info.get_expression();
            ve * ve
        } else {
            channel_info.get_volume().powf(2_f32 * volume_scale)
                * channel_info.get_expression().powf(2_f32 * expression_scale)
        };

        let mut mix_gain = self.note_gain * channel_gain * self.vol_env.get_value();
        if self.dynamic_volume || lfo_to_volume_offset != 0.0 {
            let lfo_to_volume = self.mod_lfo_to_volume + 0.1_f32 * lfo_to_volume_offset;
            let decibels = lfo_to_volume * self.mod_lfo.get_value();
            mix_gain *= SoundFontMath::decibels_to_linear(decibels);
        }
        if attenuation_offset != 0.0 {
            mix_gain *= SoundFontMath::decibels_to_linear(-0.1_f32 * attenuation_offset);
        }

        let angle =
            (consts::PI / 200_f32) * (channel_info.get_pan()
                * self.modulators.default_scale(DefaultModulator::PanToPan)
                + self.instrument_pan
                + 0.1_f32 * pan_offset
                + 50_f32);
        if angle <= 0_f32 {
            self.current_mix_gain_left = mix_gain;
            self.current_mix_gain_right = 0_f32;
        } else if angle >= SoundFontMath::HALF_PI {
            self.current_mix_gain_left = 0_f32;
            self.current_mix_gain_right = mix_gain;
        } else {
            self.current_mix_gain_left = mix_gain * angle.cos();
            self.current_mix_gain_right = mix_gain * angle.sin();
        }

        self.current_reverb_send = SoundFontMath::clamp(
            channel_info.get_reverb_send()
                * self.modulators.default_scale(DefaultModulator::ReverbSend)
                + self.instrument_reverb
                + 0.001_f32 * reverb_offset,
            0_f32,
            1_f32,
        );
        self.current_chorus_send = SoundFontMath::clamp(
            channel_info.get_chorus_send()
                * self.modulators.default_scale(DefaultModulator::ChorusSend)
                + self.instrument_chorus
                + 0.001_f32 * chorus_offset,
            0_f32,
            1_f32,
        );

        if self.voice_length == 0 {
            self.previous_mix_gain_left = self.current_mix_gain_left;
            self.previous_mix_gain_right = self.current_mix_gain_right;
            self.previous_reverb_send = self.current_reverb_send;
            self.previous_chorus_send = self.current_chorus_send;
        }

        self.voice_length += self.block.len();

        true
    }

    fn release_if_necessary(&mut self, channel_info: &Channel) {
        if self.voice_length < self.min_voice_length {
            return;
        }

        let sustained = channel_info.get_hold_pedal()
            || (self.sostenuto_captured && channel_info.get_sostenuto_pedal());

        if self.voice_state == VoiceState::ReleaseRequested && !sustained {
            self.vol_env.release();
            self.mod_env.release();
            self.oscillator.release();

            self.voice_state = VoiceState::Released;
        }
    }

    /// Called when the sostenuto pedal goes down: only notes whose key is still held are sustained.
    pub(crate) fn capture_sostenuto(&mut self) {
        self.sostenuto_captured = self.voice_state == VoiceState::Playing;
    }

    pub(crate) fn block(&self) -> &Vec<f32> {
        &self.block
    }

    pub(crate) fn voice_length(&self) -> usize {
        self.voice_length
    }

    pub(crate) fn exclusive_class(&self) -> i32 {
        self.exclusive_class
    }

    pub(crate) fn channel(&self) -> i32 {
        self.channel
    }

    pub(crate) fn key(&self) -> i32 {
        self.key
    }

    /// Decays the portamento offset towards 0 by one block and returns the offsets at the
    /// block start and end, so the oscillator interpolates the glide instead of stepping it.
    fn advance_portamento(&mut self) -> (f32, f32) {
        let start = self.portamento_offset;
        if self.portamento_speed > 0.0 && self.portamento_offset != 0.0 {
            let decay = self.portamento_speed * self.block.len() as f32;
            if self.portamento_offset > 0.0 {
                self.portamento_offset = (self.portamento_offset - decay).max(0.0);
            } else {
                self.portamento_offset = (self.portamento_offset + decay).min(0.0);
            }
        }
        (start, self.portamento_offset)
    }

    #[cfg(test)]
    /// True until the note has been asked to release.
    pub(crate) fn is_playing(&self) -> bool {
        self.voice_state == VoiceState::Playing
    }

    pub(crate) fn is_released(&self) -> bool {
        self.voice_state == VoiceState::Released
    }

    #[cfg(test)]
    pub(crate) fn filter_state(&self) -> (f32, f32, f32) {
        (self.cutoff, self.smoothed_cutoff, self.filter_q_scale)
    }

    pub(crate) fn priority(&self) -> f32 {
        if self.note_gain < SoundFontMath::NON_AUDIBLE {
            0_f32
        } else {
            self.vol_env.get_priority()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portamento_offset_is_reported_at_both_block_boundaries() {
        let settings = SynthesizerSettings::new(44100);
        let mut voice = Voice::new(&settings);
        voice.portamento_offset = -12.0;
        voice.portamento_speed = 0.5 / voice.block.len() as f32;

        assert_eq!(voice.advance_portamento(), (-12.0, -11.5));
        assert_eq!(voice.advance_portamento(), (-11.5, -11.0));

        voice.portamento_offset = 0.25;
        assert_eq!(voice.advance_portamento(), (0.25, 0.0));
    }
}
