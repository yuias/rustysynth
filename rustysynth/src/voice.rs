#![allow(dead_code)]

use std::f32::consts;

use crate::bi_quad_filter::BiQuadFilter;
use crate::channel::Channel;
use crate::lfo::Lfo;
use crate::modulation_envelope::ModulationEnvelope;
use crate::oscillator::Oscillator;
use crate::region_ex::RegionEx;
use crate::region_pair::RegionPair;
use crate::soundfont_math::SoundFontMath;
use crate::synthesizer_settings::SynthesizerSettings;
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
    instrument_reverb: f32,
    instrument_chorus: f32,

    // Some instruments require fast cutoff change, which can cause pop noise.
    // This is used to smooth out the cutoff frequency.
    smoothed_cutoff: f32,

    // Portamento: pitch offset that decays towards 0
    portamento_offset: f32,    // current pitch offset in semitones
    portamento_speed: f32,     // semitones per sample (0 = no portamento)

    voice_state: VoiceState,
    /// Time elapsed in samples
    voice_length: usize,
    min_voice_length: usize,
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
            instrument_reverb: 0_f32,
            instrument_chorus: 0_f32,
            smoothed_cutoff: 0_f32,
            portamento_offset: 0_f32,
            portamento_speed: 0_f32,
            voice_state: VoiceState::Playing,
            voice_length: 0,
            min_voice_length: (settings.sample_rate / 500) as usize,
        }
    }

    pub(crate) fn start(&mut self, region: &RegionPair, channel_info: &Channel, channel: i32, key: i32, velocity: i32,
        portamento_source: i32, portamento_speed: f32) {
        self.exclusive_class = region.get_exclusive_class();
        self.channel = channel;
        self.key = key;
        self.velocity = velocity;

        if velocity > 0 {
            // According to the Polyphone's implementation, the initial attenuation should be reduced to 40%.
            // I'm not sure why, but this indeed improves the loudness variability.
            let sample_attenuation = 0.4_f32 * region.get_initial_attenuation();
            let filter_attenuation = 0.5_f32 * region.get_initial_filter_q();
            let decibels = 2_f32 * SoundFontMath::linear_to_decibels(velocity as f32 / 127_f32)
                - sample_attenuation
                - filter_attenuation;
            self.note_gain = SoundFontMath::decibels_to_linear(decibels);
        } else {
            self.note_gain = 0_f32;
        }

        self.cutoff = region.get_initial_filter_cutoff_frequency();
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

        RegionEx::start_volume_envelope(&mut self.vol_env, region, channel_info, key, velocity);
        RegionEx::start_modulation_envelope(&mut self.mod_env, region, key, velocity);
        RegionEx::start_vibrato(&mut self.vib_lfo, region, channel_info, key, velocity);
        RegionEx::start_modulation(&mut self.mod_lfo, region, key, velocity);
        RegionEx::start_oscillator(&mut self.oscillator, region);
        self.filter.clear_buffer();
        self.filter.set_low_pass_filter(self.cutoff, self.resonance);

        self.smoothed_cutoff = self.cutoff;

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
    }

    pub(crate) fn end(&mut self) {
        if self.voice_state == VoiceState::Playing {
            self.voice_state = VoiceState::ReleaseRequested;
        }
    }

    pub(crate) fn kill(&mut self) {
        self.note_gain = 0_f32;
    }

    pub(crate) fn process(&mut self, data: &[i16], channels: &[Channel], master_tune: f32) -> bool {
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

        let vib_pitch_change = (0.01_f32 * channel_info.get_modulation() + self.vib_lfo_to_pitch)
            * self.vib_lfo.get_value();
        let mod_pitch_change = self.mod_lfo_to_pitch * self.mod_lfo.get_value()
            + self.mod_env_to_pitch * self.mod_env.get_value();
        let channel_pitch_change = channel_info.get_tune() + channel_info.get_pitch_bend();
        let scale_tuning = channel_info.get_scale_tuning_for_key(self.key);
        let pitch = self.key as f32 + self.portamento_offset + vib_pitch_change + mod_pitch_change + channel_pitch_change + master_tune + scale_tuning;

        // Decay portamento offset towards 0
        if self.portamento_speed > 0.0 && self.portamento_offset != 0.0 {
            let decay = self.portamento_speed * self.block.len() as f32;
            if self.portamento_offset > 0.0 {
                self.portamento_offset = (self.portamento_offset - decay).max(0.0);
            } else {
                self.portamento_offset = (self.portamento_offset + decay).min(0.0);
            }
        }
        if !self.oscillator.process(data, &mut self.block[..], pitch) {
            return false;
        }

        {
            // Apply CC#74 (Brightness) as cutoff offset in cents
            let brightness_cents = channel_info.get_brightness_cents();
            // Apply CC#71 (Resonance) as resonance offset in dB
            let resonance_db = channel_info.get_filter_resonance_db();

            let effective_resonance = if resonance_db != 0.0 {
                SoundFontMath::max(
                    self.resonance * SoundFontMath::decibels_to_linear(resonance_db),
                    0.001,
                )
            } else {
                self.resonance
            };

            if self.dynamic_cutoff || brightness_cents != 0.0 {
                let mod_cents = self.mod_lfo_to_cutoff as f32 * self.mod_lfo.get_value()
                    + self.mod_env_to_cutoff as f32 * self.mod_env.get_value();
                let total_cents = mod_cents + brightness_cents;
                let factor = SoundFontMath::cents_to_multiplying_factor(total_cents);
                let new_cutoff = factor * self.cutoff;

                // The cutoff change is limited within x0.5 and x2 to reduce pop noise.
                let lower_limit = 0.5_f32 * self.smoothed_cutoff;
                let upper_limit = 2_f32 * self.smoothed_cutoff;
                self.smoothed_cutoff = SoundFontMath::clamp(new_cutoff, lower_limit, upper_limit);

                self.filter
                    .set_low_pass_filter(self.smoothed_cutoff, effective_resonance);
            } else if resonance_db != 0.0 {
                self.filter
                    .set_low_pass_filter(self.cutoff, effective_resonance);
            }
        }
        self.filter.process(&mut self.block[..]);

        self.previous_mix_gain_left = self.current_mix_gain_left;
        self.previous_mix_gain_right = self.current_mix_gain_right;
        self.previous_reverb_send = self.current_reverb_send;
        self.previous_chorus_send = self.current_chorus_send;

        // According to the GM spec, the following value should be squared.
        let ve = channel_info.get_volume() * channel_info.get_expression();
        let channel_gain = ve * ve;

        let mut mix_gain = self.note_gain * channel_gain * self.vol_env.get_value();
        if self.dynamic_volume {
            let decibels = self.mod_lfo_to_volume * self.mod_lfo.get_value();
            mix_gain *= SoundFontMath::decibels_to_linear(decibels);
        }

        let angle =
            (consts::PI / 200_f32) * (channel_info.get_pan() + self.instrument_pan + 50_f32);
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
            channel_info.get_reverb_send() + self.instrument_reverb,
            0_f32,
            1_f32,
        );
        self.current_chorus_send = SoundFontMath::clamp(
            channel_info.get_chorus_send() + self.instrument_chorus,
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

        if self.voice_state == VoiceState::ReleaseRequested && !channel_info.get_hold_pedal() {
            self.vol_env.release();
            self.mod_env.release();
            self.oscillator.release();

            self.voice_state = VoiceState::Released;
        }
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

    pub(crate) fn priority(&self) -> f32 {
        if self.note_gain < SoundFontMath::NON_AUDIBLE {
            0_f32
        } else {
            self.vol_env.get_priority()
        }
    }
}
