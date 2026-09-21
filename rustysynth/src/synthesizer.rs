#![allow(dead_code)]

use std::cmp;
use std::collections::HashMap;
use std::sync::Arc;

use crate::array_math::ArrayMath;
use crate::channel::{Channel, Rx};
use crate::chorus::Chorus;
use crate::error::SynthesizerError;
use crate::master_tune::MasterTune;
use crate::region_pair::RegionPair;
use crate::reverb::Reverb;
use crate::soundfont::SoundFont;
use crate::soundfont_math::SoundFontMath;
use crate::synthesizer_settings::SynthesizerSettings;
use crate::system_mode::SystemMode;
use crate::voice_collection::VoiceCollection;

/// An instance of the SoundFont synthesizer.
#[derive(Debug)]
#[non_exhaustive]
pub struct Synthesizer {
    pub(crate) sound_font: Arc<SoundFont>,
    pub(crate) sample_rate: i32,
    pub(crate) block_size: usize,
    pub(crate) maximum_polyphony: usize,

    preset_lookup: HashMap<i32, usize>,
    default_preset: usize,

    channels: Vec<Channel>,

    voices: VoiceCollection,

    block_left: Vec<f32>,
    block_right: Vec<f32>,

    inverse_block_size: f32,

    block_read: usize,

    master_volume: f32,

    effects: Option<Effects>,

    channel_mute: u16,

    master_tune: f32,
    // Values received via Universal Real-Time SysEx. Kept apart from the API-set master
    // volume and tune so that reset() can clear them without discarding the host settings.
    sysex_master_volume: f32,
    sysex_fine_tune: f32,
    sysex_coarse_tune: f32,

    // Set by the last GM/GS/XG reset message; reset() returns it to GM.
    system_mode: SystemMode,

    enable_master_coarse_tune_on_percussion: bool,
    enable_generator_range_clamp: bool,
}

/// True when the part receives this particular control change, on top of the switch that
/// covers control changes as a whole.
///
/// Data entry is shared between RPN and NRPN, so which switch applies depends on which
/// parameter number the part is currently collecting.
fn receives_controller(channel: &Channel, controller: i32) -> bool {
    match controller {
        0x01 | 0x21 => channel.receives(Rx::Modulation),
        0x07 | 0x27 => channel.receives(Rx::Volume),
        0x0A | 0x2A => channel.receives(Rx::Panpot),
        0x0B | 0x2B => channel.receives(Rx::Expression),
        0x40 => channel.receives(Rx::Hold),
        0x05 | 0x41 | 0x54 => channel.receives(Rx::Portamento),
        0x42 => channel.receives(Rx::Sostenuto),
        0x43 => channel.receives(Rx::Soft),
        0x62 | 0x63 => channel.receives(Rx::Nrpn),
        0x64 | 0x65 => channel.receives(Rx::Rpn),
        0x06 | 0x26 => {
            if channel.is_nrpn_active() {
                channel.receives(Rx::Nrpn)
            } else {
                channel.receives(Rx::Rpn)
            }
        }
        _ => true,
    }
}

/// GS Reverb Macro (room_size, damp, width), applied with `set_reverb_room_size`,
/// `set_reverb_damp` and `set_reverb_width`. There is no delay algorithm available, so this
/// is an approximation of the GS reverb characters using only those three parameters. Macro
/// 4 (Hall 2) is the GS power-on default and maps to the reverb's built-in defaults.
const GS_REVERB_MACROS: [(f32, f32, f32); 8] = [
    (0.30, 0.70, 0.70), // 0: Room 1
    (0.38, 0.60, 0.80), // 1: Room 2
    (0.44, 0.50, 0.90), // 2: Room 3
    (0.56, 0.45, 1.00), // 3: Hall 1
    (0.50, 0.50, 1.00), // 4: Hall 2
    (0.60, 0.20, 1.00), // 5: Plate
    (0.20, 0.30, 0.40), // 6: Delay
    (0.20, 0.30, 1.00), // 7: Panning Delay
];

impl Synthesizer {
    /// The number of channels.
    pub const CHANNEL_COUNT: usize = 16;
    /// The percussion channel.
    pub const PERCUSSION_CHANNEL: usize = 9;

    /// Initializes a new synthesizer using a specified SoundFont and settings.
    ///
    /// # Arguments
    ///
    /// * `sound_font` - The SoundFont instance.
    /// * `settings` - The settings for synthesis.
    pub fn new(
        sound_font: &Arc<SoundFont>,
        settings: &SynthesizerSettings,
    ) -> Result<Self, SynthesizerError> {
        settings.validate()?;

        let mut preset_lookup: HashMap<i32, usize> = HashMap::new();

        let mut min_preset_id = i32::MAX;
        let mut default_preset: usize = 0;
        for i in 0..sound_font.presets.len() {
            let preset = &sound_font.presets[i];

            // The preset ID is Int32, where the upper 16 bits represent the bank number
            // and the lower 16 bits represent the patch number.
            // This ID is used to search for presets by the combination of bank number
            // and patch number.
            let preset_id = (preset.bank_number << 16) | preset.patch_number;
            preset_lookup.insert(preset_id, i);

            // The preset with the minimum ID number will be default.
            // If the SoundFont is GM compatible, the piano will be chosen.
            if preset_id < min_preset_id {
                default_preset = i;
                min_preset_id = preset_id;
            }
        }

        let mut channels: Vec<Channel> = Vec::new();
        for i in 0..Synthesizer::CHANNEL_COUNT {
            channels.push(Channel::new(i == Synthesizer::PERCUSSION_CHANNEL));
        }

        let voices = VoiceCollection::new(settings);

        let block_left: Vec<f32> = vec![0_f32; settings.block_size];
        let block_right: Vec<f32> = vec![0_f32; settings.block_size];

        let inverse_block_size = 1_f32 / settings.block_size as f32;

        let block_read = settings.block_size;

        let master_volume = 0.5_f32;

        let effects = if settings.enable_reverb_and_chorus {
            Some(Effects::new(settings))
        } else {
            None
        };

        Ok(Self {
            sound_font: Arc::clone(sound_font),
            sample_rate: settings.sample_rate,
            block_size: settings.block_size,
            maximum_polyphony: settings.maximum_polyphony,
            preset_lookup,
            default_preset,
            channels,
            voices,
            block_left,
            block_right,
            inverse_block_size,
            block_read,
            master_volume,
            effects,
            channel_mute: 0,
            master_tune: 0.0,
            sysex_master_volume: 1.0,
            sysex_fine_tune: 0.0,
            sysex_coarse_tune: 0.0,
            system_mode: SystemMode::Gm,
            enable_master_coarse_tune_on_percussion: settings
                .enable_master_coarse_tune_on_percussion,
            enable_generator_range_clamp: settings.enable_generator_range_clamp,
        })
    }

    /// Processes a MIDI message.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel to which the message will be sent.
    /// * `command` - The type of the message.
    /// * `data1` - The first data part of the message.
    /// * `data2` - The second data part of the message.
    pub fn process_midi_message(&mut self, channel: i32, command: i32, data1: i32, data2: i32) {
        if !(0 <= channel && channel < self.channels.len() as i32) {
            return;
        }

        let channel_info = &mut self.channels[channel as usize];

        // GS Patch Part receive switches. A message the part does not receive is discarded
        // before it reaches any state, including the raw controller array.
        let receives = match command {
            0x80 | 0x90 => channel_info.receives(Rx::Note),
            0xA0 => channel_info.receives(Rx::PolyPressure),
            0xB0 => channel_info.receives(Rx::ControlChange) && receives_controller(channel_info, data1),
            0xC0 => channel_info.receives(Rx::ProgramChange),
            0xD0 => channel_info.receives(Rx::ChannelPressure),
            0xE0 => channel_info.receives(Rx::PitchBend),
            _ => true,
        };
        if !receives {
            return;
        }

        match command {
            0x80 => self.note_off(channel, data1),       // Note Off
            0x90 => self.note_on(channel, data1, data2), // Note On
            0xA0 => channel_info.set_poly_pressure(data1, data2), // Polyphonic Key Pressure
            0xB0 => {
                channel_info.set_controller_value(data1, data2);
                match data1 {
                    0x00 => channel_info.set_bank(data2), // Bank Selection
                    0x01 => channel_info.set_modulation_coarse(data2), // Modulation Coarse
                    0x21 => channel_info.set_modulation_fine(data2), // Modulation Fine
                    0x06 => channel_info.data_entry_coarse(data2), // Data Entry Coarse
                    0x26 => channel_info.data_entry_fine(data2), // Data Entry Fine
                    0x07 => channel_info.set_volume_coarse(data2), // Channel Volume Coarse
                    0x27 => channel_info.set_volume_fine(data2), // Channel Volume Fine
                    0x0A => channel_info.set_pan_coarse(data2), // Pan Coarse
                    0x2A => channel_info.set_pan_fine(data2), // Pan Fine
                    0x0B => channel_info.set_expression_coarse(data2), // Expression Coarse
                    0x2B => channel_info.set_expression_fine(data2), // Expression Fine
                    0x20 => channel_info.set_bank_lsb(data2), // Bank Select LSB
                    0x05 => channel_info.set_portamento_time(data2), // Portamento Time
                    0x40 => channel_info.set_hold_pedal(data2), // Hold Pedal
                    0x41 => channel_info.set_portamento_on(data2), // Portamento On/Off
                    0x42 => self.set_sostenuto_pedal(channel, data2), // Sostenuto
                    0x43 => channel_info.set_soft_pedal(data2), // Soft Pedal
                    0x47 => channel_info.set_filter_resonance(data2), // Filter Resonance (CC#71)
                    0x48 => channel_info.set_release_time(data2), // Release Time (CC#72)
                    0x49 => channel_info.set_attack_time(data2), // Attack Time (CC#73)
                    0x4A => channel_info.set_brightness(data2), // Brightness (CC#74)
                    0x4B => channel_info.set_decay_time(data2), // Decay Time (CC#75)
                    0x54 => channel_info.set_portamento_control(data2), // Portamento Control
                    0x5B => channel_info.set_reverb_send(data2), // Reverb Send
                    0x5D => channel_info.set_chorus_send(data2), // Chorus Send
                    0x5E => channel_info.set_variation_send(data2), // Variation/Effect Depth
                    0x63 => channel_info.set_nrpn_coarse(data2), // NRPN Coarse
                    0x62 => channel_info.set_nrpn_fine(data2), // NRPN Fine
                    0x65 => channel_info.set_rpn_coarse(data2), // RPN Coarse
                    0x64 => channel_info.set_rpn_fine(data2), // RPN Fine
                    0x78 => self.note_off_all_channel(channel, true), // All Sound Off
                    0x79 => self.reset_all_controllers_channel(channel), // Reset All Controllers
                    0x7B => self.note_off_all_channel(channel, false), // All Note Off
                    _ => (),
                }
            }
            0xC0 => channel_info.set_patch(data1), // Program Change
            0xD0 => channel_info.set_channel_pressure(data1), // Channel Pressure
            0xE0 => channel_info.set_pitch_bend(data1, data2), // Pitch Bend
            _ => (),
        }
    }

    /// Stops a note.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel of the note.
    /// * `key` - The key of the note.
    pub fn note_off(&mut self, channel: i32, key: i32) {
        if !(0 <= channel && channel < self.channels.len() as i32) {
            return;
        }

        for voice in self.voices.get_active_voices().iter_mut() {
            if voice.channel() == channel && voice.key() == key {
                voice.end();
            }
        }
    }

    /// Starts a note.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel of the note.
    /// * `key` - The key of the note.
    /// * `velocity` - The velocity of the note.
    pub fn note_on(&mut self, channel: i32, key: i32, velocity: i32) {
        if velocity == 0 {
            self.note_off(channel, key);
            return;
        }

        if !(0 <= channel && channel < self.channels.len() as i32) {
            return;
        }

        // GS Keyboard Range: the part ignores keys outside it.
        if !self.channels[channel as usize].is_key_in_range(key) {
            return;
        }

        // Extract portamento info (&mut borrow, consumed before immutable borrow)
        let portamento_source: i32;
        let portamento_speed: f32;
        {
            let ch = &mut self.channels[channel as usize];
            portamento_source = ch.consume_portamento_source();
            portamento_speed = ch.get_portamento_speed(self.sample_rate);
            ch.set_last_note_on_key(key);
        }

        let channel_info = &self.channels[channel as usize];

        let preset_id = (channel_info.get_bank_number() << 16) | channel_info.get_patch_number();

        let mut preset = self.default_preset;
        match self.preset_lookup.get(&preset_id) {
            Some(value) => preset = *value,
            None => {
                // Try fallback to the GM sound set.
                // Normally, the given patch number + the bank number 0 will work.
                // For drums (bank number >= 128), it seems to be better to select the standard set (128:0).
                let gm_preset_id = if channel_info.get_bank_number() < 128 {
                    channel_info.get_patch_number()
                } else {
                    128 << 16
                };

                // If no corresponding preset was found. Use the default one...
                if let Some(value) = self.preset_lookup.get(&gm_preset_id) {
                    preset = *value
                }
            }
        }

        let preset = &self.sound_font.presets[preset];
        for preset_region in preset.regions.iter() {
            if preset_region.contains(key, velocity) {
                let instrument = &self.sound_font.instruments[preset_region.instrument];
                for instrument_region in instrument.regions.iter() {
                    if instrument_region.contains(key, velocity) {
                        let region_pair = RegionPair::new(
                            preset_region,
                            instrument_region,
                            self.enable_generator_range_clamp,
                        );

                        if let Some(value) = self.voices.request_new(instrument_region, channel, key) {
                            value.start(&region_pair, channel_info, channel, key, velocity,
                                portamento_source, portamento_speed)
                        }
                    }
                }
            }
        }
    }

    /// Stops all the notes in the specified channel.
    ///
    /// # Arguments
    ///
    /// * `immediate` - If `true`, notes will stop immediately without the release sound.
    pub fn note_off_all(&mut self, immediate: bool) {
        if immediate {
            self.voices.clear();
        } else {
            for voice in self.voices.get_active_voices().iter_mut() {
                voice.end();
            }
        }
    }

    /// Stops all the notes in the specified channel.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel in which the notes will be stopped.
    /// * `immediate` - If `true`, notes will stop immediately without the release sound.
    pub fn note_off_all_channel(&mut self, channel: i32, immediate: bool) {
        if immediate {
            for voice in self.voices.get_active_voices().iter_mut() {
                if voice.channel() == channel {
                    voice.kill();
                }
            }
        } else {
            for voice in self.voices.get_active_voices().iter_mut() {
                if voice.channel() == channel {
                    voice.end();
                }
            }
        }
    }

    fn set_sostenuto_pedal(&mut self, channel: i32, value: i32) {
        let channel_info = &mut self.channels[channel as usize];
        let was_down = channel_info.get_sostenuto_pedal();
        channel_info.set_sostenuto_pedal(value);

        if !was_down && channel_info.get_sostenuto_pedal() {
            for voice in self.voices.get_active_voices().iter_mut() {
                if voice.channel() == channel {
                    voice.capture_sostenuto();
                }
            }
        }
    }

    /// Resets all the controllers.
    pub fn reset_all_controllers(&mut self) {
        for channel in &mut self.channels {
            channel.reset_all_controllers();
        }
    }

    /// Resets all the controllers of the specified channel.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel to be reset.
    pub fn reset_all_controllers_channel(&mut self, channel: i32) {
        if !(0 <= channel && channel < self.channels.len() as i32) {
            return;
        }

        self.channels[channel as usize].reset_all_controllers();
    }

    /// Resets the synthesizer.
    pub fn reset(&mut self) {
        self.voices.clear();

        for channel in &mut self.channels {
            channel.reset();
        }

        if let Some(effects) = self.effects.as_mut() {
            effects.reverb.mute();
            effects.chorus.mute();
        }

        self.sysex_master_volume = 1.0;
        self.sysex_fine_tune = 0.0;
        self.sysex_coarse_tune = 0.0;

        self.block_read = self.block_size;

        self.set_system_mode(SystemMode::Gm);
    }

    /// Stores the mode and pushes it to every channel, which interprets Bank Select with it.
    fn set_system_mode(&mut self, mode: SystemMode) {
        self.system_mode = mode;
        for channel in &mut self.channels {
            channel.set_system_mode(mode);
        }
    }

    /// Handles GM/GS/XG system reset messages. Unlike `reset()`, this also restores the
    /// default drum channel assignment and selects the system mode the message defines.
    fn reset_system(&mut self, mode: SystemMode) {
        for (i, channel) in self.channels.iter_mut().enumerate() {
            channel.set_percussion_channel(i == Synthesizer::PERCUSSION_CHANNEL);
        }
        self.reset();
        self.set_system_mode(mode);
    }

    /// Renders the waveform.
    ///
    /// # Arguments
    ///
    /// * `left` - The buffer of the left channel to store the rendered waveform.
    /// * `right` - The buffer of the right channel to store the rendered waveform.
    ///
    /// # Remarks
    ///
    /// The output buffers for the left and right must be the same length.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        if left.len() != right.len() {
            panic!("The output buffers for the left and right must be the same length.");
        }

        let left_length = left.len();

        let mut wrote = 0;
        while wrote < left_length {
            if self.block_read == self.block_size {
                self.render_block();
                self.block_read = 0;
            }

            let src_rem = self.block_size - self.block_read;
            let dst_rem = left_length - wrote;
            let rem = cmp::min(src_rem, dst_rem);

            for t in 0..rem {
                left[wrote + t] = self.block_left[self.block_read + t];
                right[wrote + t] = self.block_right[self.block_read + t];
            }

            self.block_read += rem;
            wrote += rem;
        }
    }

    fn is_voice_muted(&self, voice_channel: i32) -> bool {
        voice_channel >= 0
            && (voice_channel as usize) < 16
            && (self.channel_mute & (1 << voice_channel as usize)) != 0
    }

    fn render_block(&mut self) {
        // Coarse tune transposes, so it is kept away from percussion channels unless the
        // host asked for the previous behavior.
        let master_tune = if self.enable_master_coarse_tune_on_percussion {
            MasterTune::new(
                self.master_tune + self.sysex_coarse_tune + self.sysex_fine_tune,
                0.0,
            )
        } else {
            MasterTune::new(
                self.master_tune + self.sysex_fine_tune,
                self.sysex_coarse_tune,
            )
        };
        self.voices
            .process(&self.sound_font.wave_data, &self.channels, master_tune);
        let master_volume = self.master_volume * self.sysex_master_volume;

        let channel_mute = self.channel_mute;

        self.block_left.fill(0_f32);
        self.block_right.fill(0_f32);
        for voice in self.voices.get_active_voices().iter_mut() {
            let muted = voice.channel() >= 0
                && (voice.channel() as usize) < 16
                && (channel_mute & (1 << voice.channel() as usize)) != 0;

            let vol = if muted { 0.0 } else { master_volume };

            let previous_gain_left = vol * voice.previous_mix_gain_left;
            let current_gain_left = vol * voice.current_mix_gain_left;
            Synthesizer::write_block(
                previous_gain_left,
                current_gain_left,
                voice.block(),
                &mut self.block_left[..],
                self.inverse_block_size,
            );
            let previous_gain_right = vol * voice.previous_mix_gain_right;
            let current_gain_right = vol * voice.current_mix_gain_right;
            Synthesizer::write_block(
                previous_gain_right,
                current_gain_right,
                voice.block(),
                &mut self.block_right[..],
                self.inverse_block_size,
            );
        }

        if let Some(effects) = self.effects.as_mut() {
            let chorus = &mut effects.chorus;
            let chorus_input_left = &mut effects.chorus_input_left[..];
            let chorus_input_right = &mut effects.chorus_input_right[..];
            let chorus_output_left = &mut effects.chorus_output_left[..];
            let chorus_output_right = &mut effects.chorus_output_right[..];
            chorus_input_left.fill(0_f32);
            chorus_input_right.fill(0_f32);
            for voice in self.voices.get_active_voices().iter_mut() {
                let muted = voice.channel() >= 0
                    && (voice.channel() as usize) < 16
                    && (channel_mute & (1 << voice.channel() as usize)) != 0;

                if muted {
                    continue;
                }

                let previous_gain_left = voice.previous_chorus_send * voice.previous_mix_gain_left;
                let current_gain_left = voice.current_chorus_send * voice.current_mix_gain_left;
                Synthesizer::write_block(
                    previous_gain_left,
                    current_gain_left,
                    voice.block(),
                    chorus_input_left,
                    self.inverse_block_size,
                );
                let previous_gain_right =
                    voice.previous_chorus_send * voice.previous_mix_gain_right;
                let current_gain_right = voice.current_chorus_send * voice.current_mix_gain_right;
                Synthesizer::write_block(
                    previous_gain_right,
                    current_gain_right,
                    voice.block(),
                    chorus_input_right,
                    self.inverse_block_size,
                );
            }
            chorus.process(
                chorus_input_left,
                chorus_input_right,
                chorus_output_left,
                chorus_output_right,
            );
            ArrayMath::multiply_add(
                master_volume,
                chorus_output_left,
                &mut self.block_left[..],
            );
            ArrayMath::multiply_add(
                master_volume,
                chorus_output_right,
                &mut self.block_right[..],
            );

            let reverb = &mut effects.reverb;
            let reverb_input = &mut effects.reverb_input[..];
            let reverb_output_left = &mut effects.reverb_output_left[..];
            let reverb_output_right = &mut effects.reverb_output_right[..];
            reverb_input.fill(0_f32);
            for voice in self.voices.get_active_voices().iter_mut() {
                let muted = voice.channel() >= 0
                    && (voice.channel() as usize) < 16
                    && (channel_mute & (1 << voice.channel() as usize)) != 0;

                if muted {
                    continue;
                }

                let previous_gain = reverb.get_input_gain()
                    * voice.previous_reverb_send
                    * (voice.previous_mix_gain_left + voice.previous_mix_gain_right);
                let current_gain = reverb.get_input_gain()
                    * voice.current_reverb_send
                    * (voice.current_mix_gain_left + voice.current_mix_gain_right);
                Synthesizer::write_block(
                    previous_gain,
                    current_gain,
                    voice.block(),
                    &mut reverb_input[..],
                    self.inverse_block_size,
                );
            }

            reverb.process(reverb_input, reverb_output_left, reverb_output_right);
            ArrayMath::multiply_add(
                master_volume,
                reverb_output_left,
                &mut self.block_left[..],
            );
            ArrayMath::multiply_add(
                master_volume,
                reverb_output_right,
                &mut self.block_right[..],
            );
        }
    }

    fn write_block(
        previous_gain: f32,
        current_gain: f32,
        source: &[f32],
        destination: &mut [f32],
        inverse_block_size: f32,
    ) {
        if SoundFontMath::max(previous_gain, current_gain) < SoundFontMath::NON_AUDIBLE {
            return;
        }

        if (current_gain - previous_gain).abs() < 1.0E-3_f32 {
            ArrayMath::multiply_add(current_gain, source, destination);
        } else {
            let step = inverse_block_size * (current_gain - previous_gain);
            ArrayMath::multiply_add_slope(previous_gain, step, source, destination);
        }
    }

    /// Processes a SysEx message.
    ///
    /// Supports:
    /// - Universal Non-Real-Time: GM System On (7E xx 09 01) → reset
    /// - Universal Real-Time: Master Volume (7F xx 04 01), Master Fine Tune (7F xx 04 03),
    ///   Master Coarse Tune (7F xx 04 04)
    /// - Roland GS, device IDs 10h-1Fh (41 1n 42 12 ...):
    ///   - GS Reset (40 00 7F 00) → reset
    ///   - Master Volume (40 00 04) and Master Key Shift (40 00 05), applied through the same
    ///     fields as the Universal Real-Time Master Volume/Coarse Tune messages
    ///   - Reverb Macro (40 01 30) and Reverb Level (40 01 33)
    ///   - Chorus Macro (40 01 38); GS Chorus Level (40 01 3A) is NOT supported
    ///   - Use for Rhythm Part (40 1x 15) and Scale Tuning (40 1x 40)
    /// - Yamaha XG, device IDs 10h-1Fh (43 1n 4C ...):
    ///   - XG System On (00 00 7E 00) → reset
    ///   - XG Part Mode (08 pp 07 vv) → sets/clears the percussion flag of MIDI channel pp
    ///
    /// The data slice should NOT include the leading F0 or trailing F7.
    pub fn process_sysex(&mut self, data: &[u8]) {
        if data.len() < 3 {
            return;
        }

        match data[0] {
            // Universal Non-Real-Time: GM System On
            0x7E => {
                // 7E xx 09 01 = GM System On
                if data.len() >= 3 && data[1] <= 0x7F && data[2] == 0x09 {
                    if data.len() >= 4 && (data[3] == 0x01 || data[3] == 0x02 || data[3] == 0x03) {
                        self.reset_system(SystemMode::Gm);
                    }
                }
            }
            // Universal Real-Time
            0x7F => {
                if data.len() >= 3 && data[2] == 0x04 {
                    if data.len() >= 6 && data[3] == 0x01 {
                        // Master Volume: 7F xx 04 01 ll mm
                        // Full scale is the default level, so files that send 7F 7F are not louder.
                        let volume = ((data[5] as u16) << 7 | data[4] as u16) as f32 / 16383.0;
                        self.sysex_master_volume = volume;
                    } else if data.len() >= 6 && data[3] == 0x03 {
                        // Master Fine Tune: 7F xx 04 03 ll mm
                        // 14-bit value, 0x2000 = center (no change)
                        let value = (data[5] as i32) << 7 | data[4] as i32;
                        // Range: -1 to +1 semitone (100 cents)
                        self.sysex_fine_tune = (value - 0x2000) as f32 / 8192.0;
                    } else if data.len() >= 6 && data[3] == 0x04 {
                        // Master Coarse Tune: 7F xx 04 04 00 mm
                        // mm: 0x00-0x7F, 0x40 = center (no change)
                        let semitones = data[5] as f32 - 64.0;
                        self.sysex_coarse_tune = semitones;
                    }
                }
            }
            // Roland GS
            0x41 => {
                // 41 10 42 12 ... = GS DT1 (Data Set 1)
                if data.len() >= 5
                    && (data[1] & 0xF0) == 0x10
                    && data[2] == 0x42
                    && data[3] == 0x12
                {
                    self.process_gs_sysex(&data[4..]);
                }
            }
            // Yamaha XG
            0x43 => {
                // 43 1n 4C ... = XG parameter change
                if data.len() >= 4 && (data[1] & 0xF0) == 0x10 && data[2] == 0x4C {
                    self.process_xg_sysex(&data[3..]);
                }
            }
            _ => {}
        }
    }

    /// Processes an XG parameter change payload (after 43 1n 4C header).
    fn process_xg_sysex(&mut self, addr_and_data: &[u8]) {
        if addr_and_data.len() < 4 {
            return;
        }

        let addr_high = addr_and_data[0];
        let addr_mid = addr_and_data[1];
        let addr_low = addr_and_data[2];
        let value = addr_and_data[3];

        // XG System On: 00 00 7E 00
        if addr_high == 0x00 && addr_mid == 0x00 && addr_low == 0x7E {
            if value == 0x00 {
                self.reset_system(SystemMode::Xg);
            }
            return;
        }

        // Part Mode: 08 pp 07 vv (0 = normal, >=1 = drums). pp addresses the MIDI
        // channel directly, unlike the GS part nibble.
        if addr_high == 0x08 && addr_low == 0x07 {
            if let Some(channel) = self.channels.get_mut(addr_mid as usize) {
                channel.set_percussion_channel(value != 0);
            }
        }
    }

    /// Processes GS DT1 payload (after 41 1n 42 12 header).
    fn process_gs_sysex(&mut self, addr_and_data: &[u8]) {
        if addr_and_data.len() < 3 {
            return;
        }

        let addr_high = addr_and_data[0];
        let addr_mid = addr_and_data[1];
        let addr_low = addr_and_data[2];
        // The checksum trailing the value is not validated; reading the value with `.get(3)`
        // means a message without it still works.
        let value = addr_and_data.get(3).copied();

        // GS Reset: 40 00 7F 00 [checksum]
        if addr_high == 0x40 && addr_mid == 0x00 && addr_low == 0x7F {
            if value == Some(0x00) {
                self.reset_system(SystemMode::Gs);
            }
            return;
        }

        // Master Volume: 40 00 04 vv [checksum]. Shares the Universal Real-Time field: on
        // hardware they are the same parameter, and a separate field would apply twice.
        if addr_high == 0x40 && addr_mid == 0x00 && addr_low == 0x04 {
            if let Some(vv) = value {
                self.sysex_master_volume = vv as f32 / 127.0;
            }
            return;
        }

        // Master Key Shift: 40 00 05 vv [checksum], semitones, 0x40 = no shift.
        if addr_high == 0x40 && addr_mid == 0x00 && addr_low == 0x05 {
            if let Some(vv) = value {
                self.sysex_coarse_tune = vv as f32 - 64.0;
            }
            return;
        }

        // Reverb Macro: 40 01 30 vv [checksum]. Approximates GS reverb characters with the
        // available room-size/damp/width parameters; there is no delay algorithm.
        if addr_high == 0x40 && addr_mid == 0x01 && addr_low == 0x30 {
            if let Some(vv) = value {
                if let Some(&(room_size, damp, width)) = GS_REVERB_MACROS.get(vv as usize) {
                    self.set_reverb_room_size(room_size);
                    self.set_reverb_damp(damp);
                    self.set_reverb_width(width);
                }
            }
            return;
        }

        // Reverb Level: 40 01 33 vv [checksum]. 64 is the default level.
        if addr_high == 0x40 && addr_mid == 0x01 && addr_low == 0x33 {
            if let Some(vv) = value {
                self.set_reverb_wet(vv as f32 / 64.0 * Reverb::INITIAL_WET);
            }
            return;
        }

        // Chorus Macro: 40 01 38 vv [checksum]. `set_chorus_type` sets feedback from its
        // preset and `set_chorus_params` keeps the current feedback, so for the Short Delay
        // macros the feedback call must come after `set_chorus_params`.
        if addr_high == 0x40 && addr_mid == 0x01 && addr_low == 0x38 {
            if let Some(vv) = value {
                match vv {
                    0..=5 => self.set_chorus_type(vv as i32),
                    6 => {
                        self.set_chorus_params(0.020, 0.0001, 1.0);
                        self.set_chorus_feedback(0.0);
                    }
                    7 => {
                        self.set_chorus_params(0.020, 0.0001, 1.0);
                        self.set_chorus_feedback(0.5);
                    }
                    _ => {}
                }
            }
            return;
        }

        // Part parameters that a control change can also reach. The GS documentation gives
        // each of these as equivalent to its controller, so they go through the same setters.
        if addr_high == 0x40 && (addr_mid & 0xF0) == 0x10 {
            if let Some(vv) = value {
                let midi_channel = Synthesizer::gs_part_to_channel(addr_mid);
                if midi_channel < self.channels.len() {
                    let channel = &mut self.channels[midi_channel];

                    // Receive switches: 03h-12h, in the order of the `Rx` variants.
                    const RX_SWITCHES: [Rx; 16] = [
                        Rx::PitchBend,
                        Rx::ChannelPressure,
                        Rx::ProgramChange,
                        Rx::ControlChange,
                        Rx::PolyPressure,
                        Rx::Note,
                        Rx::Rpn,
                        Rx::Nrpn,
                        Rx::Modulation,
                        Rx::Volume,
                        Rx::Panpot,
                        Rx::Expression,
                        Rx::Hold,
                        Rx::Portamento,
                        Rx::Sostenuto,
                        Rx::Soft,
                    ];
                    if (0x03..=0x12).contains(&addr_low) {
                        let rx = RX_SWITCHES[(addr_low - 0x03) as usize];
                        channel.set_rx_switch(rx, vv != 0);
                        return;
                    }

                    match addr_low {
                        // Pitch Key Shift: 28h-58h is -24 to +24 semitones.
                        0x16 => {
                            channel.set_key_shift(vv as i32 - 0x40);
                            return;
                        }
                        // Part Level, the same parameter as Channel Volume.
                        0x19 => {
                            channel.set_volume_coarse(vv as i32);
                            return;
                        }
                        // Part Panpot, the same parameter as Pan, except that 0 asks for a
                        // random position; that would make a render depend on chance, so it
                        // is taken as centre.
                        0x1C => {
                            channel.set_pan_coarse(if vv == 0 { 64 } else { vv as i32 });
                            return;
                        }
                        0x1D => {
                            channel.set_keyboard_range_low(vv as i32);
                            return;
                        }
                        0x1E => {
                            channel.set_keyboard_range_high(vv as i32);
                            return;
                        }
                        // Chorus and Reverb Send Level, the same parameters as their
                        // controllers.
                        0x21 => {
                            channel.set_chorus_send(vv as i32);
                            return;
                        }
                        0x22 => {
                            channel.set_reverb_send(vv as i32);
                            return;
                        }
                        _ => {}
                    }
                }
            }
        }

        // Use for Rhythm Part: 40 1X 15 [0 = off, 1 = map 1, 2 = map 2] [checksum]
        // Both drum maps select the drum bank, since SoundFonts have a single drum bank.
        if addr_high == 0x40 && (addr_mid & 0xF0) == 0x10 && addr_low == 0x15 {
            if let Some(&value) = addr_and_data.get(3) {
                let midi_channel = Synthesizer::gs_part_to_channel(addr_mid);
                self.channels[midi_channel].set_percussion_channel(value != 0);
            }
            return;
        }

        // Scale Tuning: 40 1X 40 [12 data bytes] [checksum]
        // X = GS part number (0-15)
        if addr_high == 0x40 && (addr_mid & 0xF0) == 0x10 && addr_low == 0x40 {
            let remaining = &addr_and_data[3..];
            if remaining.len() >= 12 {
                let midi_channel = Synthesizer::gs_part_to_channel(addr_mid);

                let mut tuning = [0.0_f32; 12];
                for i in 0..12 {
                    // 0-127, center 64 = 0 cents, each unit = 1 cent
                    tuning[i] = remaining[i] as f32 - 64.0;
                }
                if midi_channel < self.channels.len() {
                    self.channels[midi_channel].set_scale_tuning(&tuning);
                }
            }
            return;
        }
    }

    /// Maps the part nibble of a GS part address (40 1X ..) to a MIDI channel.
    /// Part 0 is channel 10, parts 1-9 are channels 1-9 and parts 10-15 are channels 11-16.
    fn gs_part_to_channel(addr_mid: u8) -> usize {
        match (addr_mid & 0x0F) as usize {
            0 => 9,
            part @ 1..=9 => part - 1,
            part => part,
        }
    }

    /// Sets the scale tuning for a specific channel.
    /// Values are in cents offset from equal temperament for each pitch class (C..B).
    pub fn set_scale_tuning(&mut self, channel: usize, tuning: &[f32; 12]) {
        if channel < self.channels.len() {
            self.channels[channel].set_scale_tuning(tuning);
        }
    }

    /// Gets the scale tuning for a specific channel.
    pub fn get_scale_tuning(&self, channel: usize) -> Option<&[f32; 12]> {
        self.channels.get(channel).map(|ch| ch.get_scale_tuning())
    }

    /// Gets the SoundFont used as the audio source.
    pub fn get_sound_font(&self) -> &SoundFont {
        &self.sound_font
    }

    /// Gets the sample rate for synthesis.
    pub fn get_sample_rate(&self) -> i32 {
        self.sample_rate
    }

    /// Gets the block size for rendering waveform.
    pub fn get_block_size(&self) -> usize {
        self.block_size
    }

    /// Gets the number of maximum polyphony.
    pub fn get_maximum_polyphony(&self) -> usize {
        self.maximum_polyphony
    }

    /// Gets the value indicating whether reverb and chorus are enabled.
    pub fn get_enable_reverb_and_chorus(&self) -> bool {
        self.effects.is_some()
    }

    /// Gets the master volume.
    /// This does not include the level received via Master Volume SysEx.
    pub fn get_master_volume(&self) -> f32 {
        self.master_volume
    }

    /// Sets the master volume.
    ///
    /// # Arguments
    ///
    /// * `value` - The new value of the master volume.
    pub fn set_master_volume(&mut self, value: f32) {
        self.master_volume = value;
    }

    /// Gets a reference to a channel by index.
    pub fn get_channel(&self, channel: usize) -> Option<&Channel> {
        self.channels.get(channel)
    }

    /// Sets the mute state for a specific channel.
    pub fn set_channel_mute(&mut self, channel: usize, muted: bool) {
        if channel < 16 {
            if muted {
                self.channel_mute |= 1 << channel;
            } else {
                self.channel_mute &= !(1 << channel);
            }
        }
    }

    /// Returns whether a specific channel is muted.
    pub fn is_channel_muted(&self, channel: usize) -> bool {
        channel < 16 && (self.channel_mute & (1 << channel)) != 0
    }

    /// Sets the channel mute mask (bitmask, bit 0 = channel 0).
    pub fn set_channel_mute_mask(&mut self, mask: u16) {
        self.channel_mute = mask;
    }

    /// Sets whether a channel is a percussion channel.
    pub fn set_percussion_channel(&mut self, channel: usize, is_percussion: bool) {
        if channel < self.channels.len() {
            self.channels[channel].set_percussion_channel(is_percussion);
        }
    }

    /// Gets the channel mute mask.
    pub fn get_channel_mute_mask(&self) -> u16 {
        self.channel_mute
    }

    /// Sets the reverb room size (0.0-1.0, default 0.5).
    pub fn set_reverb_room_size(&mut self, value: f32) {
        if let Some(effects) = self.effects.as_mut() {
            effects.reverb.set_room_size(value);
        }
    }

    /// Sets the reverb damping (0.0-1.0, default 0.5).
    pub fn set_reverb_damp(&mut self, value: f32) {
        if let Some(effects) = self.effects.as_mut() {
            effects.reverb.set_damp(value);
        }
    }

    /// Sets the reverb wet level (0.0-1.0, default ~0.33).
    pub fn set_reverb_wet(&mut self, value: f32) {
        if let Some(effects) = self.effects.as_mut() {
            effects.reverb.set_wet(value);
        }
    }

    /// Sets the reverb width (0.0-1.0, default 1.0).
    pub fn set_reverb_width(&mut self, value: f32) {
        if let Some(effects) = self.effects.as_mut() {
            effects.reverb.set_width(value);
        }
    }

    /// Sets the chorus type preset (0=Chorus1, 1=Chorus2, 2=Chorus3,
    /// 3=Chorus4, 4=FB Chorus, 5=Flanger).
    pub fn set_chorus_type(&mut self, type_id: i32) {
        if let Some(effects) = self.effects.as_mut() {
            effects.chorus.set_chorus_type(type_id);
        }
    }

    /// Sets the chorus feedback gain (0.0 to <1.0).
    pub fn set_chorus_feedback(&mut self, value: f32) {
        if let Some(effects) = self.effects.as_mut() {
            effects.chorus.set_feedback(value);
        }
    }

    /// Gets the master tuning offset in semitones.
    /// This does not include tuning received via Master Fine/Coarse Tune SysEx.
    pub fn get_master_tune(&self) -> f32 {
        self.master_tune
    }

    /// Sets the master tuning offset in semitones.
    /// For example, 1.0 = one semitone up, -0.5 = quarter tone down.
    pub fn set_master_tune(&mut self, value: f32) {
        self.master_tune = value;
    }

    /// Sets the chorus parameters.
    ///
    /// Default values: delay=0.002, depth=0.0019, frequency=0.4
    pub fn set_chorus_params(&mut self, delay: f64, depth: f64, frequency: f64) {
        if let Some(effects) = self.effects.as_mut() {
            effects
                .chorus
                .set_params(self.sample_rate, delay, depth, frequency);
        }
    }
}

#[derive(Debug)]
struct Effects {
    reverb: Reverb,
    reverb_input: Vec<f32>,
    reverb_output_left: Vec<f32>,
    reverb_output_right: Vec<f32>,

    chorus: Chorus,
    chorus_input_left: Vec<f32>,
    chorus_input_right: Vec<f32>,
    chorus_output_left: Vec<f32>,
    chorus_output_right: Vec<f32>,
}

impl Effects {
    fn new(settings: &SynthesizerSettings) -> Effects {
        Self {
            reverb: Reverb::new(settings.sample_rate),
            reverb_input: vec![0_f32; settings.block_size],
            reverb_output_left: vec![0_f32; settings.block_size],
            reverb_output_right: vec![0_f32; settings.block_size],
            chorus: Chorus::new(settings.sample_rate, 0.002, 0.0019, 0.4),
            chorus_input_left: vec![0_f32; settings.block_size],
            chorus_input_right: vec![0_f32; settings.block_size],
            chorus_output_left: vec![0_f32; settings.block_size],
            chorus_output_right: vec![0_f32; settings.block_size],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator_type::GeneratorType;
    use crate::test_util::{drum_kit_soundfont, layered_soundfont, modulated_soundfont, sine_soundfont};

    fn render_block(synthesizer: &mut Synthesizer) {
        let mut left = vec![0_f32; synthesizer.get_block_size()];
        let mut right = vec![0_f32; synthesizer.get_block_size()];
        synthesizer.render(&mut left, &mut right);
    }

    /// Renders a fixed performance touching every controller that feeds a default modulator.
    fn render_controller_performance(settings: &SynthesizerSettings) -> (f64, f64) {
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), settings).unwrap();
        let mut left = vec![0_f32; settings.block_size];
        let mut right = vec![0_f32; settings.block_size];
        let mut abs_sum = 0_f64;
        let mut square_sum = 0_f64;

        let events: [(i32, i32, i32); 12] = [
            (0xB0, 7, 90),
            (0xB0, 10, 20),
            (0xB0, 11, 100),
            (0xB0, 91, 80),
            (0xB0, 93, 60),
            (0xB0, 1, 127),
            (0xD0, 90, 0),
            (0xE0, 0, 96),
            (0xB0, 2, 50),
            (0xA0, 60, 100),
            (0xB0, 74, 30),
            (0xE0, 0, 64),
        ];

        synthesizer.note_on(0, 60, 40);
        synthesizer.note_on(0, 67, 110);
        for (i, &(command, data1, data2)) in events.iter().enumerate() {
            synthesizer.process_midi_message(0, command, data1, data2);
            for _ in 0..(8 + i) {
                synthesizer.render(&mut left, &mut right);
                for (&l, &r) in left.iter().zip(right.iter()) {
                    abs_sum += l.abs() as f64 + r.abs() as f64;
                    square_sum += (l * l) as f64 + (r * r) as f64;
                }
            }
        }
        (abs_sum, square_sum)
    }

    // Guards the default-modulator paths: a SoundFont without modulators must keep rendering
    // exactly as it did before SoundFont modulator support was added.
    #[test]
    fn soundfont_without_modulators_keeps_reference_render() {
        let settings = SynthesizerSettings::new(44100);
        let (abs_sum, square_sum) = render_controller_performance(&settings);
        assert!((abs_sum - REFERENCE_ABS_SUM).abs() <= REFERENCE_ABS_SUM * 1e-9);
        assert!((square_sum - REFERENCE_SQUARE_SUM).abs() <= REFERENCE_SQUARE_SUM * 1e-9);
    }

    const REFERENCE_ABS_SUM: f64 = 392.9663166537539;
    const REFERENCE_SQUARE_SUM: f64 = 12.505487196549511;

    #[test]
    fn stealing_does_not_take_layer_started_by_same_note_on() {
        let mut settings = SynthesizerSettings::new(44100);
        settings.maximum_polyphony = 8;
        let mut synthesizer = Synthesizer::new(&layered_soundfont(), &settings).unwrap();

        for key in 40..44 {
            synthesizer.note_on(1, key, 100);
        }
        render_block(&mut synthesizer);
        assert_eq!(synthesizer.voices.active_voices().len(), 8);

        synthesizer.note_on(0, 60, 100);
        let new_voices = synthesizer
            .voices
            .active_voices()
            .iter()
            .filter(|voice| voice.channel() == 0 && voice.key() == 60)
            .count();
        assert_eq!(new_voices, 2);
    }

    #[test]
    fn filter_returns_to_region_cutoff_when_controllers_reset_to_neutral() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();
        synthesizer.note_on(0, 60, 127);

        // CC#74 (brightness) and CC#71 (resonance) away from neutral, then back.
        synthesizer.process_midi_message(0, 0xB0, 74, 0);
        synthesizer.process_midi_message(0, 0xB0, 71, 100);
        for _ in 0..8 {
            render_block(&mut synthesizer);
        }
        let (cutoff, smoothed, q_scale) = synthesizer.voices.active_voices()[0].filter_state();
        assert!(smoothed < cutoff);
        assert!(q_scale > 1.0);

        synthesizer.process_midi_message(0, 0xB0, 74, 64);
        synthesizer.process_midi_message(0, 0xB0, 71, 64);
        for _ in 0..8 {
            render_block(&mut synthesizer);
        }
        let (cutoff, smoothed, q_scale) = synthesizer.voices.active_voices()[0].filter_state();
        assert_eq!(smoothed, cutoff);
        assert_eq!(q_scale, 1.0);
    }

    fn region_cutoff_for_velocity(enable_velocity_to_filter_cutoff: bool, velocity: i32) -> f32 {
        let mut settings = SynthesizerSettings::new(44100);
        settings.enable_velocity_to_filter_cutoff = enable_velocity_to_filter_cutoff;
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();
        synthesizer.note_on(0, 60, velocity);
        synthesizer.voices.active_voices()[0].filter_state().0
    }

    #[test]
    fn velocity_to_filter_cutoff_follows_setting() {
        let full = region_cutoff_for_velocity(true, 127);
        assert_eq!(region_cutoff_for_velocity(false, 1), full);

        // Two octaves down at velocity 0, so about 1/4 near velocity 1.
        let soft = region_cutoff_for_velocity(true, 1);
        assert!((soft / full - 0.25).abs() < 0.01, "ratio = {}", soft / full);
    }

    fn is_key_released(synthesizer: &Synthesizer, key: i32) -> bool {
        synthesizer
            .voices
            .active_voices()
            .iter()
            .find(|voice| voice.key() == key)
            .unwrap()
            .is_released()
    }

    #[test]
    fn sostenuto_sustains_only_notes_held_when_pedal_goes_down() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();

        synthesizer.note_on(0, 60, 100);
        synthesizer.process_midi_message(0, 0xB0, 66, 127);
        synthesizer.note_on(0, 64, 100);
        synthesizer.note_off(0, 60);
        synthesizer.note_off(0, 64);
        for _ in 0..4 {
            render_block(&mut synthesizer);
        }
        assert!(!is_key_released(&synthesizer, 60));
        assert!(is_key_released(&synthesizer, 64));

        // Re-sending pedal down must not capture notes started in the meantime.
        synthesizer.note_on(0, 67, 100);
        synthesizer.process_midi_message(0, 0xB0, 66, 127);
        synthesizer.note_off(0, 67);
        for _ in 0..4 {
            render_block(&mut synthesizer);
        }
        assert!(is_key_released(&synthesizer, 67));

        synthesizer.process_midi_message(0, 0xB0, 66, 0);
        render_block(&mut synthesizer);
        assert!(is_key_released(&synthesizer, 60));
    }

    #[test]
    fn instrument_modulator_overrides_default_velocity_to_cutoff() {
        // Same identity as the SF2 default, amount 0: turns the default off.
        let sound_font = modulated_soundfont(
            vec![(0x0102, GeneratorType::INITIAL_FILTER_CUTOFF_FREQUENCY, 0, 0x0D02, 0)],
            Vec::new(),
        );
        let cutoff_for = |enable_soundfont_modulators: bool| {
            let mut settings = SynthesizerSettings::new(44100);
            settings.enable_soundfont_modulators = enable_soundfont_modulators;
            let mut synthesizer = Synthesizer::new(&sound_font, &settings).unwrap();
            synthesizer.note_on(0, 60, 1);
            synthesizer.voices.active_voices()[0].filter_state().0
        };

        let full = region_cutoff_for_velocity(true, 127);
        assert_eq!(cutoff_for(true), full);
        assert!(cutoff_for(false) < 0.3 * full);
    }

    fn voice_gain(synthesizer: &Synthesizer, key: i32) -> f32 {
        let voice = synthesizer
            .voices
            .active_voices()
            .iter()
            .find(|voice| voice.key() == key)
            .unwrap();
        voice.current_mix_gain_left + voice.current_mix_gain_right
    }

    #[test]
    fn controller_modulator_changes_sounding_voice() {
        // CC2, linear, unipolar -> initial attenuation, 480 cB at full scale.
        let sound_font =
            modulated_soundfont(vec![(0x0082, GeneratorType::INITIAL_ATTENUATION, 480, 0, 0)], Vec::new());
        let settings = SynthesizerSettings::new(44100);
        let mut reference = Synthesizer::new(&sound_font, &settings).unwrap();
        let mut modulated = Synthesizer::new(&sound_font, &settings).unwrap();
        for synthesizer in [&mut reference, &mut modulated] {
            synthesizer.note_on(0, 60, 100);
            render_block(synthesizer);
        }
        assert_eq!(voice_gain(&modulated, 60), voice_gain(&reference, 60));

        modulated.process_midi_message(0, 0xB0, 2, 127);
        render_block(&mut reference);
        render_block(&mut modulated);

        // 480 cB * 127/128 is about 47.6 dB.
        let db = 20.0 * (voice_gain(&modulated, 60) / voice_gain(&reference, 60)).log10();
        assert!((db + 47.625).abs() < 0.01, "db = {}", db);
    }

    #[test]
    fn poly_pressure_modulator_affects_only_its_key() {
        let sound_font =
            modulated_soundfont(vec![(0x000A, GeneratorType::INITIAL_ATTENUATION, 960, 0, 0)], Vec::new());
        let settings = SynthesizerSettings::new(44100);
        let mut reference = Synthesizer::new(&sound_font, &settings).unwrap();
        let mut modulated = Synthesizer::new(&sound_font, &settings).unwrap();
        for synthesizer in [&mut reference, &mut modulated] {
            synthesizer.note_on(0, 60, 100);
            synthesizer.note_on(0, 72, 100);
            render_block(synthesizer);
        }

        modulated.process_midi_message(0, 0xA0, 60, 100);
        render_block(&mut reference);
        render_block(&mut modulated);
        assert!(voice_gain(&modulated, 60) < 0.1 * voice_gain(&reference, 60));
        assert_eq!(voice_gain(&modulated, 72), voice_gain(&reference, 72));
    }

    #[test]
    fn velocity_modulator_changes_note_on_destination() {
        // Velocity -> attack time: +15875 timecents at full velocity, from 1 ms to about 9 s.
        let sound_font = modulated_soundfont(
            vec![(0x0002, GeneratorType::ATTACK_VOLUME_ENVELOPE, 16000, 0, 0)],
            Vec::new(),
        );
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sound_font, &settings).unwrap();
        synthesizer.note_on(0, 60, 127);
        synthesizer.note_on(0, 72, 40);
        for _ in 0..16 {
            render_block(&mut synthesizer);
        }
        assert!(voice_gain(&synthesizer, 60) < 0.1 * voice_gain(&synthesizer, 72));
    }

    #[test]
    fn gs_reset_restores_default_drum_channel() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();
        synthesizer.set_percussion_channel(9, false);
        synthesizer.set_percussion_channel(10, true);

        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41]);
        assert_eq!(synthesizer.channels[9].get_bank_number(), 128);
        assert_eq!(synthesizer.channels[10].get_bank_number(), 0);
    }

    #[test]
    fn gs_use_for_rhythm_part_switches_drum_channel() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();

        // Part 1 is channel 1 (index 0); part 0 is channel 10.
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x11, 0x15, 0x02, 0x18]);
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x10, 0x15, 0x00, 0x1B]);
        assert_eq!(synthesizer.channels[0].get_bank_number(), 128);
        assert_eq!(synthesizer.channels[9].get_bank_number(), 0);
    }

    #[test]
    fn plain_reset_keeps_configured_drum_channel() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();
        synthesizer.set_percussion_channel(10, true);

        synthesizer.reset();
        assert_eq!(synthesizer.channels[10].get_bank_number(), 128);
    }

    #[test]
    fn reset_messages_select_system_mode_and_reset_returns_to_gm() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();

        let assert_mode_pushed = |synthesizer: &Synthesizer, mode: SystemMode| {
            assert_eq!(synthesizer.system_mode, mode);
            assert_eq!(synthesizer.channels[0].get_system_mode(), mode);
            assert_eq!(synthesizer.channels[15].get_system_mode(), mode);
        };

        assert_mode_pushed(&synthesizer, SystemMode::Gm);

        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41]);
        assert_mode_pushed(&synthesizer, SystemMode::Gs);

        synthesizer.process_sysex(&[0x43, 0x10, 0x4C, 0x00, 0x00, 0x7E, 0x00]);
        assert_mode_pushed(&synthesizer, SystemMode::Xg);

        synthesizer.process_sysex(&[0x7E, 0x7F, 0x09, 0x01]);
        assert_mode_pushed(&synthesizer, SystemMode::Gm);

        synthesizer.process_sysex(&[0x43, 0x10, 0x4C, 0x00, 0x00, 0x7E, 0x00]);
        assert_mode_pushed(&synthesizer, SystemMode::Xg);
        synthesizer.reset();
        assert_mode_pushed(&synthesizer, SystemMode::Gm);
    }

    #[test]
    fn gs_and_xg_device_ids_10_to_1f_are_accepted() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();
        synthesizer.set_percussion_channel(9, false);

        synthesizer.process_sysex(&[0x41, 0x1F, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41]);
        assert_eq!(synthesizer.system_mode, SystemMode::Gs);
        assert_eq!(synthesizer.channels[9].get_bank_number(), 128);

        synthesizer.process_sysex(&[0x43, 0x1F, 0x4C, 0x00, 0x00, 0x7E, 0x00]);
        assert_eq!(synthesizer.system_mode, SystemMode::Xg);

        // Device ID 0x20 falls outside the accepted 10h-1Fh range, so the message is ignored.
        synthesizer.process_sysex(&[0x41, 0x20, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41]);
        assert_eq!(synthesizer.system_mode, SystemMode::Xg);
    }

    #[test]
    fn xg_bank_msb_127_selects_drum_bank_and_keeps_patch() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();

        synthesizer.process_sysex(&[0x43, 0x10, 0x4C, 0x00, 0x00, 0x7E, 0x00]);

        synthesizer.process_midi_message(0, 0xB0, 0, 127);
        synthesizer.process_midi_message(0, 0xC0, 5, 0);
        assert_eq!(synthesizer.channels[0].get_bank_number(), 128);
        assert_eq!(synthesizer.channels[0].get_patch_number(), 5);

        synthesizer.process_midi_message(0, 0xB0, 0, 0);
        assert_eq!(synthesizer.channels[0].get_bank_number(), 0);
        assert!(!synthesizer.channels[0].get_is_percussion_channel());
    }

    #[test]
    fn xg_part_mode_sysex_sets_and_clears_percussion() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();

        // No reset message is sent, so this also proves Part Mode works in GM mode.
        synthesizer.process_sysex(&[0x43, 0x10, 0x4C, 0x08, 0x02, 0x07, 0x01]);
        assert!(synthesizer.channels[2].get_is_percussion_channel());

        synthesizer.process_sysex(&[0x43, 0x10, 0x4C, 0x08, 0x02, 0x07, 0x00]);
        assert!(!synthesizer.channels[2].get_is_percussion_channel());

        synthesizer.process_sysex(&[0x43, 0x1F, 0x4C, 0x08, 0x09, 0x07, 0x00]);
        assert!(!synthesizer.channels[9].get_is_percussion_channel());

        // Out-of-range part index is ignored without panicking.
        synthesizer.process_sysex(&[0x43, 0x10, 0x4C, 0x08, 0x10, 0x07, 0x01]);
    }

    #[test]
    fn master_fine_and_coarse_tune_sysex_add_up() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();
        synthesizer.set_master_tune(0.25);

        synthesizer.process_sysex(&[0x7F, 0x7F, 0x04, 0x04, 0x00, 0x42]);
        synthesizer.process_sysex(&[0x7F, 0x7F, 0x04, 0x03, 0x00, 0x60]);
        assert_eq!(synthesizer.sysex_coarse_tune, 2.0);
        assert_eq!(synthesizer.sysex_fine_tune, 0.5);

        // reset() is called by the sequencer before playback: it must drop the values from
        // the previous file but keep the host's own tuning.
        synthesizer.reset();
        assert_eq!(synthesizer.sysex_coarse_tune, 0.0);
        assert_eq!(synthesizer.sysex_fine_tune, 0.0);
        assert_eq!(synthesizer.get_master_tune(), 0.25);
    }

    #[test]
    fn full_scale_master_volume_sysex_keeps_default_level() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();
        let volume = synthesizer.get_master_volume();

        synthesizer.process_sysex(&[0x7F, 0x7F, 0x04, 0x01, 0x7F, 0x7F]);
        assert_eq!(synthesizer.master_volume * synthesizer.sysex_master_volume, volume);

        synthesizer.process_sysex(&[0x7F, 0x7F, 0x04, 0x01, 0x00, 0x00]);
        assert_eq!(synthesizer.sysex_master_volume, 0.0);
        synthesizer.reset();
        assert_eq!(synthesizer.sysex_master_volume, 1.0);
    }

    #[test]
    fn truncated_master_volume_sysex_is_ignored() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();
        synthesizer.process_sysex(&[0x7F, 0x7F, 0x04, 0x01, 0x7F]);
        assert_eq!(synthesizer.sysex_master_volume, 1.0);
    }

    /// Renders one note on `channel` after optionally applying a master tuning SysEx.
    fn render_tuned_note(
        settings: &SynthesizerSettings,
        channel: i32,
        sysex: Option<&[u8]>,
    ) -> Vec<f32> {
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), settings).unwrap();
        if let Some(sysex) = sysex {
            synthesizer.process_sysex(sysex);
        }
        synthesizer.note_on(channel, 60, 100);

        let mut left = vec![0_f32; 1024];
        let mut right = vec![0_f32; 1024];
        synthesizer.render(&mut left, &mut right);
        left
    }

    // Master Coarse Tune: 7F 7F 04 04 00 42 = +2 semitones.
    const COARSE_TUNE_UP: &[u8] = &[0x7F, 0x7F, 0x04, 0x04, 0x00, 0x42];
    // Master Fine Tune: 7F 7F 04 03 00 60 = +half a semitone.
    const FINE_TUNE_UP: &[u8] = &[0x7F, 0x7F, 0x04, 0x03, 0x00, 0x60];

    #[test]
    fn master_coarse_tune_leaves_percussion_channels_alone() {
        let settings = SynthesizerSettings::new(44100);

        // Channel 10 is percussion by default: transposing it would change which instrument
        // each key plays, so the rendered block must be identical.
        let plain = render_tuned_note(&settings, 9, None);
        let tuned = render_tuned_note(&settings, 9, Some(COARSE_TUNE_UP));
        assert_eq!(plain, tuned);

        // A melodic channel is still transposed.
        let plain = render_tuned_note(&settings, 0, None);
        let tuned = render_tuned_note(&settings, 0, Some(COARSE_TUNE_UP));
        assert_ne!(plain, tuned);
    }

    #[test]
    fn master_fine_tune_still_reaches_percussion_channels() {
        let settings = SynthesizerSettings::new(44100);

        let plain = render_tuned_note(&settings, 9, None);
        let tuned = render_tuned_note(&settings, 9, Some(FINE_TUNE_UP));
        assert_ne!(plain, tuned);
    }

    #[test]
    fn master_tune_api_still_reaches_percussion_channels() {
        let settings = SynthesizerSettings::new(44100);
        let plain = render_tuned_note(&settings, 9, None);

        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();
        synthesizer.set_master_tune(2.0);
        synthesizer.note_on(9, 60, 100);
        let mut left = vec![0_f32; 1024];
        let mut right = vec![0_f32; 1024];
        synthesizer.render(&mut left, &mut right);

        assert_ne!(plain, left);
    }

    #[test]
    fn master_coarse_tune_on_percussion_setting_restores_the_previous_behavior() {
        let mut settings = SynthesizerSettings::new(44100);
        settings.enable_master_coarse_tune_on_percussion = true;

        let plain = render_tuned_note(&settings, 9, None);
        let tuned = render_tuned_note(&settings, 9, Some(COARSE_TUNE_UP));
        assert_ne!(plain, tuned);
    }

    #[test]
    fn velocity_to_cutoff_modulator_is_in_effect_from_the_first_block() {
        use crate::test_util::SoundFontBuilder;
        use std::io::Cursor;

        // A modulator offset against a region that simply declares the resulting cutoff.
        // Both must sound the same from the first sample: routing the offset through the
        // per-block path would ramp the cutoff down over the first blocks instead of
        // starting there. A linear 7-bit source maps velocity 127 to 127/128, so the
        // offset is -9600 * 127/128 = -9525 from the 13500 default.
        fn font(cutoff: i16, modulators: Vec<(u16, u16, i16, u16, u16)>) -> Arc<SoundFont> {
            let mut builder = SoundFontBuilder::new();
            let wave: Vec<i16> = (0..64)
                .map(|i| ((i as f64 * std::f64::consts::TAU / 64.0).sin() * 12000.0) as i16)
                .collect();
            let sample = builder.sample("Sine", &wave, 44100, 60, 0, 64);
            let instrument = builder.instrument_with_modulators(
                "Filtered",
                None,
                vec![(
                    vec![
                        (GeneratorType::SAMPLE_MODES, 1),
                        (GeneratorType::INITIAL_FILTER_CUTOFF_FREQUENCY, cutoff),
                    ],
                    modulators,
                    sample,
                )],
            );
            builder.preset_with_modulators("Filtered", 0, 0, None, vec![(Vec::new(), Vec::new(), instrument)]);
            let mut cursor = Cursor::new(builder.build());
            Arc::new(SoundFont::new(&mut cursor).unwrap())
        }

        fn render(sound_font: &Arc<SoundFont>) -> Vec<f32> {
            let settings = SynthesizerSettings::new(44100);
            let mut synthesizer = Synthesizer::new(sound_font, &settings).unwrap();
            synthesizer.note_on(0, 60, 127);
            let mut left = vec![0_f32; 1024];
            let mut right = vec![0_f32; 1024];
            synthesizer.render(&mut left, &mut right);
            left
        }

        let modulated = render(&font(
            13500,
            vec![(
                0x0002,
                GeneratorType::INITIAL_FILTER_CUTOFF_FREQUENCY,
                -9600,
                0,
                0,
            )],
        ));
        let declared = render(&font(3975, Vec::new()));

        assert_eq!(modulated, declared);
    }

    #[test]
    fn a_modulator_cannot_push_a_generator_past_its_spec_range() {
        // +9600 cents on top of the 13500 default would put the cutoff far above the
        // 13500 maximum, so the region must sound exactly like an unmodulated one.
        let modulated = modulated_soundfont(
            vec![(
                0x0002,
                GeneratorType::INITIAL_FILTER_CUTOFF_FREQUENCY,
                9600,
                0,
                0,
            )],
            Vec::new(),
        );
        let plain = modulated_soundfont(Vec::new(), Vec::new());

        let render = |sound_font: &Arc<SoundFont>| {
            let settings = SynthesizerSettings::new(44100);
            let mut synthesizer = Synthesizer::new(sound_font, &settings).unwrap();
            synthesizer.note_on(0, 60, 127);
            let mut left = vec![0_f32; 512];
            let mut right = vec![0_f32; 512];
            synthesizer.render(&mut left, &mut right);
            left
        };

        assert_eq!(render(&modulated), render(&plain));
    }

    #[test]
    fn generator_range_clamp_setting_restores_the_unclamped_behavior() {
        // +9600 cents on top of the 13500 default puts the cutoff past the 13500 maximum.
        let modulated = modulated_soundfont(
            vec![(
                0x0002,
                GeneratorType::INITIAL_FILTER_CUTOFF_FREQUENCY,
                9600,
                0,
                0,
            )],
            Vec::new(),
        );
        let plain = modulated_soundfont(Vec::new(), Vec::new());

        let render = |sound_font: &Arc<SoundFont>, clamp: bool| {
            let mut settings = SynthesizerSettings::new(44100);
            settings.enable_generator_range_clamp = clamp;
            let mut synthesizer = Synthesizer::new(sound_font, &settings).unwrap();
            synthesizer.note_on(0, 60, 127);
            let mut left = vec![0_f32; 512];
            let mut right = vec![0_f32; 512];
            synthesizer.render(&mut left, &mut right);
            left
        };

        // Clamped, the modulated region sounds like the unmodulated one.
        assert_eq!(render(&modulated, true), render(&plain, true));
        // Unclamped, the cutoff runs past the maximum and the region sounds different.
        assert_ne!(render(&modulated, false), render(&plain, false));
    }

    #[test]
    fn gs_drum_nrpns_reach_the_voice() {
        // NRPN coarse is CC#99, NRPN fine is CC#98, data entry coarse is CC#6.
        let send_drum_nrpn = |synthesizer: &mut Synthesizer, msb: i32, note: i32, value: i32| {
            synthesizer.process_midi_message(9, 0xB0, 0x63, msb);
            synthesizer.process_midi_message(9, 0xB0, 0x62, note);
            synthesizer.process_midi_message(9, 0xB0, 0x06, value);
        };

        let render = |nrpn: Option<(i32, i32)>| {
            let settings = SynthesizerSettings::new(44100);
            let mut synthesizer = Synthesizer::new(&drum_kit_soundfont(), &settings).unwrap();
            if let Some((msb, value)) = nrpn {
                send_drum_nrpn(&mut synthesizer, msb, 40, value);
            }
            synthesizer.note_on(9, 40, 100);
            let mut left = vec![0_f32; 1024];
            let mut right = vec![0_f32; 1024];
            synthesizer.render(&mut left, &mut right);
            (left, right)
        };

        let peak = |block: &[f32]| block.iter().fold(0_f32, |a, s| a.max(s.abs()));
        let (plain_left, plain_right) = render(None);

        // Level 64 of 127 scales the note.
        let (level_left, _) = render(Some((0x1A, 64)));
        let ratio = peak(&level_left) / peak(&plain_left);
        assert!(
            (ratio - 64.0 / 127.0).abs() < 1e-3,
            "level ratio was {}",
            ratio
        );

        // Pan 127 is hard right, so the left channel goes silent.
        let (pan_left, pan_right) = render(Some((0x1C, 127)));
        assert!(peak(&pan_left) < 1e-6);
        assert!(peak(&pan_right) > peak(&plain_right));

        // A pitch offset changes the waveform.
        let (pitch_left, _) = render(Some((0x18, 66)));
        assert_ne!(pitch_left, plain_left);
    }

    #[test]
    fn gs_part_parameters_reach_the_same_state_as_their_controllers() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();

        // Part 2 is MIDI channel 1: 41 1n 42 12 40 1x nn vv.
        let part = |synthesizer: &mut Synthesizer, nn: u8, vv: u8| {
            synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x12, nn, vv]);
        };

        let mut reference = Synthesizer::new(&sine_soundfont(), &settings).unwrap();
        reference.process_midi_message(1, 0xB0, 0x07, 90); // Channel Volume
        reference.process_midi_message(1, 0xB0, 0x0A, 100); // Pan
        reference.process_midi_message(1, 0xB0, 0x5B, 70); // Reverb Send
        reference.process_midi_message(1, 0xB0, 0x5D, 30); // Chorus Send

        part(&mut synthesizer, 0x19, 90);
        part(&mut synthesizer, 0x1C, 100);
        part(&mut synthesizer, 0x22, 70);
        part(&mut synthesizer, 0x21, 30);

        assert_eq!(
            synthesizer.channels[1].get_volume(),
            reference.channels[1].get_volume()
        );
        assert_eq!(
            synthesizer.channels[1].get_pan(),
            reference.channels[1].get_pan()
        );
        assert_eq!(
            synthesizer.channels[1].get_reverb_send(),
            reference.channels[1].get_reverb_send()
        );
        assert_eq!(
            synthesizer.channels[1].get_chorus_send(),
            reference.channels[1].get_chorus_send()
        );

        // Pitch Key Shift is additive with the RPN coarse tune.
        part(&mut synthesizer, 0x16, 0x40 + 3);
        assert_eq!(synthesizer.channels[1].get_tune(), 3.0);

        // A panpot of 0 asks for a random position and is taken as centre, which is the
        // same state as the centre value of the controller.
        part(&mut synthesizer, 0x1C, 0);
        reference.process_midi_message(1, 0xB0, 0x0A, 64);
        assert_eq!(
            synthesizer.channels[1].get_pan(),
            reference.channels[1].get_pan()
        );
    }

    #[test]
    fn gs_keyboard_range_silences_keys_outside_it() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();

        // Part 1 is MIDI channel 0: keyboard range 60-72.
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x11, 0x1D, 60]);
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x11, 0x1E, 72]);

        synthesizer.note_on(0, 59, 100);
        assert_eq!(synthesizer.voices.active_voices().len(), 0);

        synthesizer.note_on(0, 60, 100);
        assert_eq!(synthesizer.voices.active_voices().len(), 1);

        synthesizer.note_on(0, 73, 100);
        assert_eq!(synthesizer.voices.active_voices().len(), 1);
    }

    #[test]
    fn gs_receive_switches_discard_the_messages_they_cover() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();

        // Part 1 is MIDI channel 0: 41 1n 42 12 40 11 nn vv.
        let rx = |synthesizer: &mut Synthesizer, nn: u8, on: u8| {
            synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x11, nn, on]);
        };

        // Rx. NOTE MESSAGE off silences the part.
        rx(&mut synthesizer, 0x08, 0);
        synthesizer.process_midi_message(0, 0x90, 60, 100);
        assert_eq!(synthesizer.voices.active_voices().len(), 0);
        rx(&mut synthesizer, 0x08, 1);
        synthesizer.process_midi_message(0, 0x90, 60, 100);
        assert_eq!(synthesizer.voices.active_voices().len(), 1);

        // Rx. VOLUME off leaves the channel volume alone, and the raw controller with it.
        let volume = synthesizer.channels[0].get_volume();
        rx(&mut synthesizer, 0x0C, 0);
        synthesizer.process_midi_message(0, 0xB0, 0x07, 10);
        assert_eq!(synthesizer.channels[0].get_volume(), volume);
        assert_eq!(synthesizer.channels[0].get_controller_value(0x07), 100);

        // Another controller still gets through, so the gate is per parameter.
        synthesizer.process_midi_message(0, 0xB0, 0x0A, 100);
        assert_eq!(synthesizer.channels[0].get_controller_value(0x0A), 100);

        // Rx. CONTROL CHANGE off blocks every controller.
        rx(&mut synthesizer, 0x06, 0);
        synthesizer.process_midi_message(0, 0xB0, 0x0A, 20);
        assert_eq!(synthesizer.channels[0].get_controller_value(0x0A), 100);
        rx(&mut synthesizer, 0x06, 1);

        // Rx. NRPN off blocks the parameter number and the data entry that follows it,
        // while RPN keeps working.
        rx(&mut synthesizer, 0x0A, 0);
        synthesizer.process_midi_message(0, 0xB0, 0x63, 0x01);
        synthesizer.process_midi_message(0, 0xB0, 0x62, 0x20);
        synthesizer.process_midi_message(0, 0xB0, 0x06, 100);
        assert_eq!(synthesizer.channels[0].get_brightness_raw(), 64);

        synthesizer.process_midi_message(0, 0xB0, 0x65, 0);
        synthesizer.process_midi_message(0, 0xB0, 0x64, 0);
        synthesizer.process_midi_message(0, 0xB0, 0x06, 4);
        assert_eq!(synthesizer.channels[0].get_pitch_bend_range(), 4.0);

        // Rx. PITCH BEND off freezes the bend.
        rx(&mut synthesizer, 0x03, 0);
        synthesizer.process_midi_message(0, 0xE0, 0, 100);
        assert_eq!(synthesizer.channels[0].get_pitch_bend(), 0.0);
    }

    #[test]
    fn gs_master_volume_and_key_shift_use_sysex_fields() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();

        // GS Master Volume: 40 00 04 vv
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x04, 0x40]);
        assert!((synthesizer.sysex_master_volume - 64.0 / 127.0).abs() < 1e-5);

        // GS Master Key Shift: 40 00 05 vv, 0x42 = +2 semitones.
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x05, 0x42]);
        assert_eq!(synthesizer.sysex_coarse_tune, 2.0);

        synthesizer.reset();
        assert_eq!(synthesizer.sysex_master_volume, 1.0);
        assert_eq!(synthesizer.sysex_coarse_tune, 0.0);
    }

    #[test]
    fn gs_reverb_macro_sets_effect_parameters() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();

        let assert_reverb = |synthesizer: &Synthesizer, room_size: f32, damp: f32, width: f32| {
            let reverb = &synthesizer.effects.as_ref().unwrap().reverb;
            assert!((reverb.get_room_size() - room_size).abs() < 1e-5);
            assert!((reverb.get_damp() - damp).abs() < 1e-5);
            assert!((reverb.get_width() - width).abs() < 1e-5);
        };

        // GS Reverb Macro 0 (Room 1): 40 01 30 00
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x01, 0x30, 0x00]);
        assert_reverb(&synthesizer, 0.30, 0.70, 0.70);

        // GS Reverb Macro 4 (Hall 2): 40 01 30 04
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x01, 0x30, 0x04]);
        assert_reverb(&synthesizer, 0.50, 0.50, 1.00);

        // Out-of-range macro 8 is ignored, so the previous parameters are unchanged.
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x01, 0x30, 0x08]);
        assert_reverb(&synthesizer, 0.50, 0.50, 1.00);
    }

    #[test]
    fn xg_bank_lsb_selects_melodic_bank_only_in_xg_mode() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();

        synthesizer.process_sysex(&[0x43, 0x10, 0x4C, 0x00, 0x00, 0x7E, 0x00]); // XG System On
        synthesizer.process_midi_message(0, 0xB0, 0, 0);
        synthesizer.process_midi_message(0, 0xB0, 32, 5);
        assert_eq!(synthesizer.channels[0].get_bank_number(), 5);

        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41]); // GS Reset
        synthesizer.process_midi_message(0, 0xB0, 0, 0);
        synthesizer.process_midi_message(0, 0xB0, 32, 5);
        assert_eq!(synthesizer.channels[0].get_bank_number(), 0);

        synthesizer.process_sysex(&[0x7E, 0x7F, 0x09, 0x01]); // GM System On
        synthesizer.process_midi_message(0, 0xB0, 0, 0);
        synthesizer.process_midi_message(0, 0xB0, 32, 5);
        assert_eq!(synthesizer.channels[0].get_bank_number(), 0);
    }

    #[test]
    fn xg_drum_kit_note_on_uses_bank_128_with_patch() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&drum_kit_soundfont(), &settings).unwrap();

        synthesizer.process_sysex(&[0x43, 0x10, 0x4C, 0x00, 0x00, 0x7E, 0x00]); // XG System On
        synthesizer.process_midi_message(0, 0xB0, 0, 127);
        synthesizer.process_midi_message(0, 0xC0, 5, 0);

        // Key 60 falls in preset 128:5's range (60-127); a wrong bank (255 or 0:0's
        // full-range preset) would start 0 or 2 voices instead of exactly 1.
        synthesizer.note_on(0, 60, 100);
        assert_eq!(synthesizer.voices.active_voices().len(), 1);

        // Preset 128:5 has no region for key 40, so this note-on starts no new voice.
        synthesizer.note_on(0, 40, 100);
        assert_eq!(synthesizer.voices.active_voices().len(), 1);
    }

    #[test]
    fn gs_chorus_macro_sets_effect_parameters() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();

        let assert_chorus = |synthesizer: &Synthesizer, delay: f64, feedback: f32| {
            let chorus = &synthesizer.effects.as_ref().unwrap().chorus;
            assert!((chorus.get_delay() - delay).abs() < 1e-5);
            assert!((chorus.get_feedback() - feedback).abs() < 1e-5);
        };

        // GS Chorus Macro 4 (Feedback Chorus): 40 01 38 04
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x01, 0x38, 0x04]);
        assert_chorus(&synthesizer, 0.008, 0.5);

        // GS Chorus Macro 5 (Flanger): 40 01 38 05
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x01, 0x38, 0x05]);
        assert_chorus(&synthesizer, 0.002, 0.7);

        // GS Chorus Macro 0 (Chorus 1): 40 01 38 00
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x01, 0x38, 0x00]);
        assert_chorus(&synthesizer, 0.006, 0.0);

        // GS Chorus Macro 7 (Short Delay FB): 40 01 38 07
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x01, 0x38, 0x07]);
        assert_chorus(&synthesizer, 0.020, 0.5);

        // GS Chorus Macro 6 (Short Delay): 40 01 38 06
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x01, 0x38, 0x06]);
        assert_chorus(&synthesizer, 0.020, 0.0);

        // Out-of-range macro 8 is ignored, so the previous parameters are unchanged.
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x01, 0x38, 0x08]);
        assert_chorus(&synthesizer, 0.020, 0.0);
    }

    #[test]
    fn gs_reverb_level_scales_wet_level() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();

        let get_wet =
            |synthesizer: &Synthesizer| synthesizer.effects.as_ref().unwrap().reverb.get_wet();

        // GS Reverb Level 64 (default): 40 01 33 40
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x01, 0x33, 0x40]);
        assert!((get_wet(&synthesizer) - Reverb::INITIAL_WET).abs() < 1e-5);

        // GS Reverb Level 127: 40 01 33 7F
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x01, 0x33, 0x7F]);
        assert!((get_wet(&synthesizer) - 127.0 / 64.0 * Reverb::INITIAL_WET).abs() < 1e-5);

        // GS Reverb Level 0: 40 01 33 00
        synthesizer.process_sysex(&[0x41, 0x10, 0x42, 0x12, 0x40, 0x01, 0x33, 0x00]);
        assert_eq!(get_wet(&synthesizer), 0.0);
    }

    #[test]
    fn xg_system_on_restores_default_drum_channel() {
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();
        synthesizer.set_percussion_channel(9, false);
        synthesizer.set_percussion_channel(10, true);

        synthesizer.process_sysex(&[0x43, 0x10, 0x4C, 0x00, 0x00, 0x7E, 0x00]); // XG System On
        assert_eq!(synthesizer.channels[9].get_bank_number(), 128);
        assert_eq!(synthesizer.channels[10].get_bank_number(), 0);
    }
}
