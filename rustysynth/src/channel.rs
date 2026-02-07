#![allow(dead_code)]

#[derive(Debug, PartialEq, Eq)]
enum DataType {
    None,
    Rpn,
    Nrpn,
}

#[derive(Debug)]
#[non_exhaustive]
pub struct Channel {
    pub(crate) is_percussion_channel: bool,

    bank_number: i32,
    patch_number: i32,

    modulation: i16,
    volume: i16,
    pan: i16,
    expression: i16,
    hold_pedal: bool,

    reverb_send: u8,
    chorus_send: u8,

    rpn: i16,
    nrpn: i16,
    pitch_bend_range: i16,
    coarse_tune: i16,
    fine_tune: i16,

    pitch_bend: f32,

    last_data_type: DataType,

    // Phase 2: Additional CC fields
    bank_lsb: i32,
    sostenuto_pedal: bool,
    soft_pedal: bool,
    variation_send: u8,

    // Sound Controller CCs (GM2/GS/XG)
    // Stored as raw 0-127 values, 64 = no change
    filter_resonance: u8, // CC#71
    release_time: u8,     // CC#72
    attack_time: u8,      // CC#73
    brightness: u8,       // CC#74
    decay_time: u8,       // CC#75

    // NRPN vibrato parameters (GS/XG)
    // Stored as raw 0-127 values, 64 = no change
    vibrato_rate: u8,  // NRPN MSB=1, LSB=8
    vibrato_depth: u8, // NRPN MSB=1, LSB=9
    vibrato_delay: u8, // NRPN MSB=1, LSB=10

    // Scale tuning: per-octave pitch offset in cents for each pitch class (C..B)
    // Default: all 0.0 (equal temperament)
    scale_tuning: [f32; 12],

    // Portamento
    portamento_on: bool,         // CC#65
    portamento_time: u8,         // CC#5 (raw 0-127)
    portamento_control: i32,     // CC#84 (source key, -1 = use last note)
    last_note_on_key: i32,       // Tracks the last note-on key for portamento source
}

impl Channel {
    pub(crate) fn new(is_percussion_channel: bool) -> Self {
        let mut channel = Self {
            is_percussion_channel,
            bank_number: 0,
            patch_number: 0,
            modulation: 0,
            volume: 0,
            pan: 0,
            expression: 0,
            hold_pedal: false,
            reverb_send: 0,
            chorus_send: 0,
            rpn: 0,
            nrpn: -1,
            pitch_bend_range: 0,
            coarse_tune: 0,
            fine_tune: 0,
            pitch_bend: 0_f32,
            last_data_type: DataType::None,
            bank_lsb: 0,
            sostenuto_pedal: false,
            soft_pedal: false,
            variation_send: 0,
            filter_resonance: 64,
            release_time: 64,
            attack_time: 64,
            brightness: 64,
            decay_time: 64,
            vibrato_rate: 64,
            vibrato_depth: 64,
            vibrato_delay: 64,
            scale_tuning: [0.0; 12],
            portamento_on: false,
            portamento_time: 0,
            portamento_control: -1,
            last_note_on_key: -1,
        };

        channel.reset();

        channel
    }

    pub(crate) fn reset(&mut self) {
        self.bank_number = if self.is_percussion_channel { 128 } else { 0 };
        self.patch_number = 0;

        self.modulation = 0;
        self.volume = 100 << 7;
        self.pan = 64 << 7;
        self.expression = 127 << 7;
        self.hold_pedal = false;

        self.reverb_send = 40;
        self.chorus_send = 0;

        self.rpn = -1;
        self.pitch_bend_range = 2 << 7;
        self.coarse_tune = 0;
        self.fine_tune = 8192;

        self.pitch_bend = 0_f32;

        self.bank_lsb = 0;
        self.sostenuto_pedal = false;
        self.soft_pedal = false;
        self.variation_send = 0;
        self.filter_resonance = 64;
        self.release_time = 64;
        self.attack_time = 64;
        self.brightness = 64;
        self.decay_time = 64;
        self.nrpn = -1;
        self.vibrato_rate = 64;
        self.vibrato_depth = 64;
        self.vibrato_delay = 64;
        self.scale_tuning = [0.0; 12];
        self.portamento_on = false;
        self.portamento_time = 0;
        self.portamento_control = -1;
        self.last_note_on_key = -1;
    }

    pub(crate) fn reset_all_controllers(&mut self) {
        self.modulation = 0;
        self.expression = 127 << 7;
        self.hold_pedal = false;

        self.rpn = -1;

        self.pitch_bend = 0_f32;

        self.sostenuto_pedal = false;
        self.soft_pedal = false;
        self.portamento_on = false;
        self.portamento_control = -1;
    }

    pub(crate) fn set_bank(&mut self, value: i32) {
        self.bank_number = value;

        if self.is_percussion_channel {
            self.bank_number += 128;
        }
    }

    pub(crate) fn set_patch(&mut self, value: i32) {
        self.patch_number = value;
    }

    pub(crate) fn set_modulation_coarse(&mut self, value: i32) {
        self.modulation = (self.modulation & 0x7F) | (value << 7) as i16;
    }

    pub(crate) fn set_modulation_fine(&mut self, value: i32) {
        self.modulation = (((self.modulation as i32) & 0xFF80) | value) as i16;
    }

    pub(crate) fn set_volume_coarse(&mut self, value: i32) {
        self.volume = (self.volume & 0x7F) | (value << 7) as i16;
    }

    pub(crate) fn set_volume_fine(&mut self, value: i32) {
        self.volume = (((self.volume as i32) & 0xFF80) | value) as i16;
    }

    pub(crate) fn set_pan_coarse(&mut self, value: i32) {
        self.pan = (self.pan & 0x7F) | (value << 7) as i16;
    }

    pub(crate) fn set_pan_fine(&mut self, value: i32) {
        self.pan = (((self.pan as i32) & 0xFF80) | value) as i16;
    }

    pub(crate) fn set_expression_coarse(&mut self, value: i32) {
        self.expression = (self.expression & 0x7F) | (value << 7) as i16;
    }

    pub(crate) fn set_expression_fine(&mut self, value: i32) {
        self.expression = (((self.expression as i32) & 0xFF80) | value) as i16;
    }

    pub(crate) fn set_hold_pedal(&mut self, value: i32) {
        self.hold_pedal = value >= 64;
    }

    pub(crate) fn set_reverb_send(&mut self, value: i32) {
        self.reverb_send = value as u8;
    }

    pub(crate) fn set_chorus_send(&mut self, value: i32) {
        self.chorus_send = value as u8;
    }

    pub(crate) fn set_rpn_coarse(&mut self, value: i32) {
        self.rpn = (self.rpn & 0x7F) | (value << 7) as i16;
        self.last_data_type = DataType::Rpn;
    }

    pub(crate) fn set_rpn_fine(&mut self, value: i32) {
        self.rpn = (((self.rpn as i32) & 0xFF80) | value) as i16;
        self.last_data_type = DataType::Rpn;
    }

    pub(crate) fn set_nrpn_coarse(&mut self, value: i32) {
        self.nrpn = (self.nrpn & 0x7F) | (value << 7) as i16;
        self.last_data_type = DataType::Nrpn;
    }

    pub(crate) fn set_nrpn_fine(&mut self, value: i32) {
        self.nrpn = (((self.nrpn as i32) & 0xFF80) | value) as i16;
        self.last_data_type = DataType::Nrpn;
    }

    pub(crate) fn data_entry_coarse(&mut self, value: i32) {
        match self.last_data_type {
            DataType::Rpn => {
                if self.rpn == 0 {
                    self.pitch_bend_range =
                        (self.pitch_bend_range & 0x7F) | (value << 7) as i16;
                } else if self.rpn == 1 {
                    self.fine_tune = (self.fine_tune & 0x7F) | (value << 7) as i16;
                } else if self.rpn == 2 {
                    self.coarse_tune = (value - 64) as i16;
                }
            }
            DataType::Nrpn => {
                self.nrpn_data_entry_coarse(value);
            }
            DataType::None => {}
        }
    }

    pub(crate) fn data_entry_fine(&mut self, value: i32) {
        match self.last_data_type {
            DataType::Rpn => {
                if self.rpn == 0 {
                    self.pitch_bend_range =
                        (((self.pitch_bend_range as i32) & 0xFF80) | value) as i16;
                } else if self.rpn == 1 {
                    self.fine_tune = (((self.fine_tune as i32) & 0xFF80) | value) as i16;
                }
            }
            DataType::Nrpn => {
                // NRPN fine data entry is not used for GS/XG tone parameters
            }
            DataType::None => {}
        }
    }

    // NRPN MSB=1: GS/XG tone modify parameters
    // Address = (MSB << 7) | LSB
    const NRPN_VIBRATO_RATE: i16 = (1 << 7) | 8;     // MSB=1, LSB=8
    const NRPN_VIBRATO_DEPTH: i16 = (1 << 7) | 9;    // MSB=1, LSB=9
    const NRPN_VIBRATO_DELAY: i16 = (1 << 7) | 10;   // MSB=1, LSB=10
    const NRPN_TVF_CUTOFF: i16 = (1 << 7) | 32;      // MSB=1, LSB=32
    const NRPN_TVF_RESONANCE: i16 = (1 << 7) | 33;   // MSB=1, LSB=33
    const NRPN_TVA_ATTACK: i16 = (1 << 7) | 99;      // MSB=1, LSB=99
    const NRPN_TVA_DECAY: i16 = (1 << 7) | 100;      // MSB=1, LSB=100
    const NRPN_TVA_RELEASE: i16 = (1 << 7) | 102;    // MSB=1, LSB=102

    fn nrpn_data_entry_coarse(&mut self, value: i32) {
        match self.nrpn {
            Self::NRPN_VIBRATO_RATE => self.vibrato_rate = value as u8,
            Self::NRPN_VIBRATO_DEPTH => self.vibrato_depth = value as u8,
            Self::NRPN_VIBRATO_DELAY => self.vibrato_delay = value as u8,
            Self::NRPN_TVF_CUTOFF => self.brightness = value as u8,
            Self::NRPN_TVF_RESONANCE => self.filter_resonance = value as u8,
            Self::NRPN_TVA_ATTACK => self.attack_time = value as u8,
            Self::NRPN_TVA_DECAY => self.decay_time = value as u8,
            Self::NRPN_TVA_RELEASE => self.release_time = value as u8,
            _ => {}
        }
    }

    pub(crate) fn set_pitch_bend(&mut self, value1: i32, value2: i32) {
        self.pitch_bend = (1_f32 / 8192_f32) * ((value1 | (value2 << 7)) - 8192) as f32;
    }

    // Phase 2: Additional CC setters
    pub(crate) fn set_bank_lsb(&mut self, value: i32) {
        self.bank_lsb = value;
    }

    pub(crate) fn set_sostenuto_pedal(&mut self, value: i32) {
        self.sostenuto_pedal = value >= 64;
    }

    pub(crate) fn set_soft_pedal(&mut self, value: i32) {
        self.soft_pedal = value >= 64;
    }

    pub(crate) fn set_variation_send(&mut self, value: i32) {
        self.variation_send = value as u8;
    }

    // Sound Controller CC setters
    pub(crate) fn set_filter_resonance(&mut self, value: i32) {
        self.filter_resonance = value as u8;
    }

    pub(crate) fn set_release_time(&mut self, value: i32) {
        self.release_time = value as u8;
    }

    pub(crate) fn set_attack_time(&mut self, value: i32) {
        self.attack_time = value as u8;
    }

    pub(crate) fn set_brightness(&mut self, value: i32) {
        self.brightness = value as u8;
    }

    pub(crate) fn set_decay_time(&mut self, value: i32) {
        self.decay_time = value as u8;
    }

    // Public getters (normalized values)
    pub fn get_bank_number(&self) -> i32 {
        self.bank_number
    }

    pub fn get_patch_number(&self) -> i32 {
        self.patch_number
    }

    pub fn get_modulation(&self) -> f32 {
        (50_f32 / 16383_f32) * self.modulation as f32
    }

    pub fn get_volume(&self) -> f32 {
        (1_f32 / 16383_f32) * self.volume as f32
    }

    pub fn get_pan(&self) -> f32 {
        (100_f32 / 16383_f32) * self.pan as f32 - 50_f32
    }

    pub fn get_expression(&self) -> f32 {
        (1_f32 / 16383_f32) * self.expression as f32
    }

    pub fn get_hold_pedal(&self) -> bool {
        self.hold_pedal
    }

    pub fn get_reverb_send(&self) -> f32 {
        (1_f32 / 127_f32) * self.reverb_send as f32
    }

    pub fn get_chorus_send(&self) -> f32 {
        (1_f32 / 127_f32) * self.chorus_send as f32
    }

    pub fn get_pitch_bend_range(&self) -> f32 {
        (self.pitch_bend_range >> 7) as f32 + 0.01_f32 * (self.pitch_bend_range & 0x7F) as f32
    }

    pub fn get_tune(&self) -> f32 {
        self.coarse_tune as f32 + (1_f32 / 8192_f32) * (self.fine_tune - 8192) as f32
    }

    pub fn get_pitch_bend(&self) -> f32 {
        self.get_pitch_bend_range() * self.pitch_bend
    }

    // Raw getters (0-127 range for UI display)
    pub fn get_volume_raw(&self) -> u8 {
        (self.volume >> 7) as u8
    }

    pub fn get_pan_raw(&self) -> u8 {
        (self.pan >> 7) as u8
    }

    pub fn get_expression_raw(&self) -> u8 {
        (self.expression >> 7) as u8
    }

    pub fn get_reverb_send_raw(&self) -> u8 {
        self.reverb_send
    }

    pub fn get_chorus_send_raw(&self) -> u8 {
        self.chorus_send
    }

    pub fn get_is_percussion_channel(&self) -> bool {
        self.is_percussion_channel
    }

    // Phase 2: Additional CC getters
    pub fn get_bank_lsb(&self) -> i32 {
        self.bank_lsb
    }

    pub fn get_sostenuto_pedal(&self) -> bool {
        self.sostenuto_pedal
    }

    pub fn get_soft_pedal(&self) -> bool {
        self.soft_pedal
    }

    pub fn get_variation_send(&self) -> f32 {
        (1_f32 / 127_f32) * self.variation_send as f32
    }

    pub fn get_variation_send_raw(&self) -> u8 {
        self.variation_send
    }

    // Sound Controller CC getters
    /// Returns the filter resonance offset in cents relative to 64.
    /// Range: 0-127 (64 = no change).
    pub fn get_filter_resonance_raw(&self) -> u8 {
        self.filter_resonance
    }

    /// Returns the release time offset as raw value.
    /// Range: 0-127 (64 = no change).
    pub fn get_release_time_raw(&self) -> u8 {
        self.release_time
    }

    /// Returns the attack time offset as raw value.
    /// Range: 0-127 (64 = no change).
    pub fn get_attack_time_raw(&self) -> u8 {
        self.attack_time
    }

    /// Returns the brightness (cutoff) offset as raw value.
    /// Range: 0-127 (64 = no change).
    pub fn get_brightness_raw(&self) -> u8 {
        self.brightness
    }

    /// Returns the decay time offset as raw value.
    /// Range: 0-127 (64 = no change).
    pub fn get_decay_time_raw(&self) -> u8 {
        self.decay_time
    }

    /// Returns the brightness offset in cents for filter cutoff.
    /// 64 = 0 cents (no change), each unit = ~50 cents.
    pub(crate) fn get_brightness_cents(&self) -> f32 {
        (self.brightness as f32 - 64.0) * 50.0
    }

    /// Returns the resonance offset in dB.
    /// 64 = 0 dB (no change), each unit = ~0.5 dB.
    pub(crate) fn get_filter_resonance_db(&self) -> f32 {
        (self.filter_resonance as f32 - 64.0) * 0.5
    }

    /// Returns the attack time multiplier (timecents-based).
    /// 64 = 1.0 (no change).
    pub(crate) fn get_attack_time_multiplier(&self) -> f32 {
        if self.attack_time == 64 {
            1.0
        } else {
            // Each unit = ~50 timecents offset
            let timecents = (self.attack_time as f32 - 64.0) * 50.0;
            2_f32.powf(timecents / 1200.0)
        }
    }

    /// Returns the decay time multiplier (timecents-based).
    /// 64 = 1.0 (no change).
    pub(crate) fn get_decay_time_multiplier(&self) -> f32 {
        if self.decay_time == 64 {
            1.0
        } else {
            let timecents = (self.decay_time as f32 - 64.0) * 50.0;
            2_f32.powf(timecents / 1200.0)
        }
    }

    /// Returns the release time multiplier (timecents-based).
    /// 64 = 1.0 (no change).
    pub(crate) fn get_release_time_multiplier(&self) -> f32 {
        if self.release_time == 64 {
            1.0
        } else {
            let timecents = (self.release_time as f32 - 64.0) * 50.0;
            2_f32.powf(timecents / 1200.0)
        }
    }

    // NRPN vibrato getters (raw)
    pub fn get_vibrato_rate_raw(&self) -> u8 {
        self.vibrato_rate
    }

    pub fn get_vibrato_depth_raw(&self) -> u8 {
        self.vibrato_depth
    }

    pub fn get_vibrato_delay_raw(&self) -> u8 {
        self.vibrato_delay
    }

    /// Returns the vibrato rate multiplier.
    /// 64 = 1.0 (no change). Each unit = ~50 timecents.
    pub(crate) fn get_vibrato_rate_multiplier(&self) -> f32 {
        if self.vibrato_rate == 64 {
            1.0
        } else {
            let timecents = (self.vibrato_rate as f32 - 64.0) * 50.0;
            2_f32.powf(timecents / 1200.0)
        }
    }

    /// Returns the vibrato depth multiplier.
    /// 64 = 1.0 (no change). Each unit scales the depth linearly.
    pub(crate) fn get_vibrato_depth_multiplier(&self) -> f32 {
        if self.vibrato_depth == 64 {
            1.0
        } else {
            // Linear scaling: 0 = 0x, 64 = 1x, 127 = ~2x
            self.vibrato_depth as f32 / 64.0
        }
    }

    /// Returns the vibrato delay multiplier.
    /// 64 = 1.0 (no change). Each unit = ~50 timecents.
    pub(crate) fn get_vibrato_delay_multiplier(&self) -> f32 {
        if self.vibrato_delay == 64 {
            1.0
        } else {
            let timecents = (self.vibrato_delay as f32 - 64.0) * 50.0;
            2_f32.powf(timecents / 1200.0)
        }
    }

    // Portamento setters
    pub(crate) fn set_portamento_on(&mut self, value: i32) {
        self.portamento_on = value >= 64;
    }

    pub(crate) fn set_portamento_time(&mut self, value: i32) {
        self.portamento_time = value as u8;
    }

    pub(crate) fn set_portamento_control(&mut self, value: i32) {
        self.portamento_control = value;
    }

    pub(crate) fn set_last_note_on_key(&mut self, key: i32) {
        self.last_note_on_key = key;
    }

    // Portamento getters
    pub fn get_portamento_on(&self) -> bool {
        self.portamento_on
    }

    pub fn get_portamento_time_raw(&self) -> u8 {
        self.portamento_time
    }

    /// Returns the portamento source key for the next note-on.
    /// If CC#84 was set, uses that value (one-shot); otherwise uses last_note_on_key.
    /// Returns -1 if no source is available.
    pub(crate) fn consume_portamento_source(&mut self) -> i32 {
        if self.portamento_control >= 0 {
            let source = self.portamento_control;
            self.portamento_control = -1; // one-shot: consumed after use
            source
        } else {
            self.last_note_on_key
        }
    }

    /// Computes portamento speed in semitones per sample.
    /// Returns 0.0 if portamento is off or time is 0.
    pub(crate) fn get_portamento_speed(&self, sample_rate: i32) -> f32 {
        if !self.portamento_on || self.portamento_time == 0 {
            return 0.0;
        }
        // Exponential mapping: t=1 → ~15ms/octave, t=127 → ~10s/octave
        let t = self.portamento_time as f32;
        let one_octave_seconds = 0.01 * 2_f32.powf(t * 10.0 / 127.0);
        let semitones_per_second = 12.0 / one_octave_seconds;
        semitones_per_second / sample_rate as f32
    }

    // Scale tuning

    /// Sets the scale tuning for all 12 pitch classes (C..B).
    /// Values are in cents offset from equal temperament.
    pub(crate) fn set_scale_tuning(&mut self, tuning: &[f32; 12]) {
        self.scale_tuning = *tuning;
    }

    /// Gets the scale tuning array (12 pitch classes, cents).
    pub fn get_scale_tuning(&self) -> &[f32; 12] {
        &self.scale_tuning
    }

    /// Returns the scale tuning offset in semitones for a given MIDI key.
    pub(crate) fn get_scale_tuning_for_key(&self, key: i32) -> f32 {
        let pitch_class = (key % 12) as usize;
        self.scale_tuning[pitch_class] * 0.01 // cents to semitones
    }
}
