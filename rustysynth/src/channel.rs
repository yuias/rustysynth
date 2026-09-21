#![allow(dead_code)]

use crate::system_mode::SystemMode;

/// A GS Patch Part receive switch, as the bit it occupies in `Channel::rx_switches`. The
/// order follows the GS parameter addresses 03h to 12h.
#[derive(Clone, Copy)]
pub(crate) enum Rx {
    PitchBend = 0,
    ChannelPressure = 1,
    ProgramChange = 2,
    ControlChange = 3,
    PolyPressure = 4,
    Note = 5,
    Rpn = 6,
    Nrpn = 7,
    Modulation = 8,
    Volume = 9,
    Panpot = 10,
    Expression = 11,
    Hold = 12,
    Portamento = 13,
    Sostenuto = 14,
    Soft = 15,
}

/// Every receive switch on.
const ALL_RX_SWITCHES: u16 = u16::MAX;

/// Marks a GS drum instrument parameter the file has not set, so that the value from the
/// SoundFont is used instead. The parameters themselves only reach 127.
const UNSET_DRUM_PARAMETER: u8 = 0xFF;

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
    // GS Pitch Key Shift, a per-part transposition separate from the RPN coarse tune.
    key_shift: i16,
    // GS Patch Part receive switches, one bit per `Rx`. All on unless a GS message says
    // otherwise, so a file that sends none behaves as though the switches did not exist.
    rx_switches: u16,
    // GS Keyboard Range: notes outside it are not sounded by this part.
    keyboard_range_low: u8,
    keyboard_range_high: u8,
    fine_tune: i16,

    pitch_bend: f32,

    last_data_type: DataType,

    // Controllers stored for inspection; not all have an audible effect yet
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

    // GS drum instrument NRPNs, indexed by note number and used only while the channel is a
    // percussion channel. 0xFF marks a parameter the file never set, so the SoundFont value
    // stands; the pitch offset is relative and needs no sentinel.
    drum_pitch_coarse: [u8; 128], // NRPN MSB=0x18, 0x40 = no change
    drum_level: [u8; 128],        // NRPN MSB=0x1A
    drum_pan: [u8; 128],          // NRPN MSB=0x1C
    drum_reverb_send: [u8; 128],  // NRPN MSB=0x1D
    drum_chorus_send: [u8; 128],  // NRPN MSB=0x1E

    // Scale tuning: per-octave pitch offset in cents for each pitch class (C..B)
    // Default: all 0.0 (equal temperament)
    scale_tuning: [f32; 12],

    // Channel pressure (aftertouch)
    channel_pressure: u8,

    // Portamento
    portamento_on: bool,         // CC#65
    portamento_time: u8,         // CC#5 (raw 0-127)
    portamento_control: i32,     // CC#84 (source key, -1 = use last note)
    last_note_on_key: i32,       // Tracks the last note-on key for portamento source

    // Raw values of every controller, poly pressure and the pitch wheel, read by SoundFont
    // modulators whose sources are not covered by the typed fields above.
    controller_values: [u8; 128],
    poly_pressure: [u8; 128],
    pitch_bend_raw: u16,

    // Pushed by the synthesizer on every mode change; decides how the bank
    // select pair maps to the SoundFont bank.
    system_mode: SystemMode,
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
            key_shift: 0,
            rx_switches: ALL_RX_SWITCHES,
            keyboard_range_low: 0,
            keyboard_range_high: 127,
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
            channel_pressure: 0,
            portamento_on: false,
            portamento_time: 0,
            portamento_control: -1,
            last_note_on_key: -1,
            controller_values: [0; 128],
            poly_pressure: [0; 128],
            pitch_bend_raw: 8192,
            system_mode: SystemMode::Gm,
            drum_pitch_coarse: [64; 128],
            drum_level: [UNSET_DRUM_PARAMETER; 128],
            drum_pan: [UNSET_DRUM_PARAMETER; 128],
            drum_reverb_send: [UNSET_DRUM_PARAMETER; 128],
            drum_chorus_send: [UNSET_DRUM_PARAMETER; 128],
        };

        channel.reset();

        channel
    }

    pub(crate) fn reset(&mut self) {
        self.bank_number = 0;
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
        self.key_shift = 0;
        self.rx_switches = ALL_RX_SWITCHES;
        self.keyboard_range_low = 0;
        self.keyboard_range_high = 127;
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
        self.drum_pitch_coarse = [64; 128];
        self.drum_level = [UNSET_DRUM_PARAMETER; 128];
        self.drum_pan = [UNSET_DRUM_PARAMETER; 128];
        self.drum_reverb_send = [UNSET_DRUM_PARAMETER; 128];
        self.drum_chorus_send = [UNSET_DRUM_PARAMETER; 128];
        self.scale_tuning = [0.0; 12];
        self.channel_pressure = 0;
        self.portamento_on = false;
        self.portamento_time = 0;
        self.portamento_control = -1;
        self.last_note_on_key = -1;

        self.controller_values = [0; 128];
        self.controller_values[7] = 100;
        self.controller_values[10] = 64;
        self.controller_values[11] = 127;
        self.controller_values[91] = 40;
        self.controller_values[71..=75].fill(64);
        self.poly_pressure = [0; 128];
        self.pitch_bend_raw = 8192;
    }

    pub(crate) fn set_percussion_channel(&mut self, is_percussion: bool) {
        self.is_percussion_channel = is_percussion;
    }

    /// Pitch offset in semitones from the GS drum instrument NRPN, 0 when unset.
    pub(crate) fn get_drum_pitch_coarse(&self, key: i32) -> f32 {
        match self.drum_note(key) {
            Some(note) => self.drum_pitch_coarse[note] as f32 - 64.0,
            None => 0.0,
        }
    }

    /// Level from the GS drum instrument NRPN as a gain, `None` when unset.
    pub(crate) fn get_drum_level(&self, key: i32) -> Option<f32> {
        self.drum_parameter(&self.drum_level, key)
            .map(|value| value as f32 / 127.0)
    }

    /// Pan from the GS drum instrument NRPN in the same units as the pan generator
    /// (-50 left to 50 right), `None` when unset.
    ///
    /// A value of 0 asks for a random pan, which would make a render depend on chance; it is
    /// treated as centre instead.
    pub(crate) fn get_drum_pan(&self, key: i32) -> Option<f32> {
        self.drum_parameter(&self.drum_pan, key)
            .map(|value| match value {
                0 => 0.0,
                _ => (value as f32 - 64.0) * (50.0 / 63.0),
            })
    }

    /// Reverb send from the GS drum instrument NRPN, `None` when unset.
    pub(crate) fn get_drum_reverb_send(&self, key: i32) -> Option<f32> {
        self.drum_parameter(&self.drum_reverb_send, key)
            .map(|value| value as f32 / 127.0)
    }

    /// Chorus send from the GS drum instrument NRPN, `None` when unset.
    pub(crate) fn get_drum_chorus_send(&self, key: i32) -> Option<f32> {
        self.drum_parameter(&self.drum_chorus_send, key)
            .map(|value| value as f32 / 127.0)
    }

    /// The array index for a key, if the drum parameters apply to this channel at all.
    fn drum_note(&self, key: i32) -> Option<usize> {
        if !self.is_percussion_channel {
            return None;
        }
        usize::try_from(key).ok().filter(|note| *note < 128)
    }

    fn drum_parameter(&self, values: &[u8; 128], key: i32) -> Option<u8> {
        self.drum_note(key)
            .map(|note| values[note])
            .filter(|value| *value != UNSET_DRUM_PARAMETER)
    }

    /// GS Pitch Key Shift, in semitones.
    pub(crate) fn set_key_shift(&mut self, semitones: i32) {
        self.key_shift = semitones.clamp(-24, 24) as i16;
    }

    /// Sets one GS Patch Part receive switch.
    pub(crate) fn set_rx_switch(&mut self, rx: Rx, on: bool) {
        let bit = 1 << rx as u16;
        if on {
            self.rx_switches |= bit;
        } else {
            self.rx_switches &= !bit;
        }
    }

    /// True when the part acts on the messages this switch covers.
    pub(crate) fn receives(&self, rx: Rx) -> bool {
        self.rx_switches & (1 << rx as u16) != 0
    }

    /// True when the part is currently collecting an NRPN rather than an RPN.
    pub(crate) fn is_nrpn_active(&self) -> bool {
        self.last_data_type == DataType::Nrpn
    }

    /// GS Keyboard Range. A low bound above the high bound silences the part, which is what
    /// the parameter is for.
    pub(crate) fn set_keyboard_range_low(&mut self, value: i32) {
        self.keyboard_range_low = value.clamp(0, 127) as u8;
    }

    pub(crate) fn set_keyboard_range_high(&mut self, value: i32) {
        self.keyboard_range_high = value.clamp(0, 127) as u8;
    }

    /// True when the part sounds this key at all.
    pub(crate) fn is_key_in_range(&self, key: i32) -> bool {
        key >= self.keyboard_range_low as i32 && key <= self.keyboard_range_high as i32
    }

    pub(crate) fn set_system_mode(&mut self, mode: SystemMode) {
        self.system_mode = mode;
    }

    pub(crate) fn get_system_mode(&self) -> SystemMode {
        self.system_mode
    }

    pub(crate) fn reset_all_controllers(&mut self) {
        self.modulation = 0;
        self.expression = 127 << 7;
        self.hold_pedal = false;

        // RP-015: both parameter numbers become null, so a later data entry is ignored.
        self.rpn = -1;
        self.nrpn = -1;
        self.last_data_type = DataType::None;

        self.pitch_bend = 0_f32;

        self.sostenuto_pedal = false;
        self.soft_pedal = false;
        self.channel_pressure = 0;
        self.portamento_on = false;
        self.portamento_control = -1;

        self.controller_values[1] = 0;
        self.controller_values[11] = 127;
        self.controller_values[64..=67].fill(0);
        self.poly_pressure = [0; 128];
        self.pitch_bend_raw = 8192;
    }

    pub(crate) fn set_bank(&mut self, value: i32) {
        self.bank_number = value;
        // XG selects drum kits by Bank MSB 127 (126 for SFX kits); GM/GS use the
        // percussion flag directly, so leave it untouched there.
        if self.system_mode == SystemMode::Xg {
            self.is_percussion_channel = value == 0x7F || value == 0x7E;
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

    // NRPN MSB=0x18-0x1E: GS drum instrument parameters. The NRPN LSB is the note number
    // rather than a parameter number, so these are matched on the MSB alone.
    const NRPN_DRUM_PITCH_COARSE: i16 = 0x18;
    const NRPN_DRUM_LEVEL: i16 = 0x1A;
    const NRPN_DRUM_PAN: i16 = 0x1C;
    const NRPN_DRUM_REVERB_SEND: i16 = 0x1D;
    const NRPN_DRUM_CHORUS_SEND: i16 = 0x1E;

    fn nrpn_data_entry_coarse(&mut self, value: i32) {
        if self.nrpn >= 0 {
            let note = (self.nrpn & 0x7F) as usize;
            let value = value.clamp(0, 127) as u8;
            match self.nrpn >> 7 {
                Self::NRPN_DRUM_PITCH_COARSE => {
                    self.drum_pitch_coarse[note] = value;
                    return;
                }
                Self::NRPN_DRUM_LEVEL => {
                    self.drum_level[note] = value;
                    return;
                }
                Self::NRPN_DRUM_PAN => {
                    self.drum_pan[note] = value;
                    return;
                }
                Self::NRPN_DRUM_REVERB_SEND => {
                    self.drum_reverb_send[note] = value;
                    return;
                }
                Self::NRPN_DRUM_CHORUS_SEND => {
                    self.drum_chorus_send[note] = value;
                    return;
                }
                _ => {}
            }
        }

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
        self.pitch_bend_raw = ((value1 & 0x7F) | ((value2 & 0x7F) << 7)) as u16;
    }

    pub(crate) fn set_controller_value(&mut self, controller: i32, value: i32) {
        if (0..128).contains(&controller) {
            self.controller_values[controller as usize] = (value & 0x7F) as u8;
        }
    }

    pub(crate) fn get_controller_value(&self, controller: u8) -> u8 {
        self.controller_values[(controller & 0x7F) as usize]
    }

    pub(crate) fn set_poly_pressure(&mut self, key: i32, value: i32) {
        if (0..128).contains(&key) {
            self.poly_pressure[key as usize] = (value & 0x7F) as u8;
        }
    }

    pub(crate) fn get_poly_pressure(&self, key: i32) -> u8 {
        if (0..128).contains(&key) {
            self.poly_pressure[key as usize]
        } else {
            0
        }
    }

    pub(crate) fn get_pitch_bend_raw(&self) -> u16 {
        self.pitch_bend_raw
    }

    // Setters for controllers without dedicated handling above
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
    /// Returns the bank number used for preset lookup.
    ///
    /// In GM and GS mode this is the Bank Select MSB, plus 128 on percussion channels.
    /// In XG mode (after an XG System On message) melodic channels use the Bank Select
    /// LSB and percussion channels return exactly 128, because XG selects drum kits with
    /// MSB 127 (126 for SFX kits) and variation banks with the LSB.
    pub fn get_bank_number(&self) -> i32 {
        if self.system_mode == SystemMode::Xg {
            if self.is_percussion_channel {
                128
            } else {
                self.bank_lsb
            }
        } else if self.is_percussion_channel {
            self.bank_number + 128
        } else {
            self.bank_number
        }
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
        self.coarse_tune as f32
            + self.key_shift as f32
            + (1_f32 / 8192_f32) * (self.fine_tune - 8192) as f32
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

    // Getters for controllers without dedicated handling above
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

    // Channel pressure
    pub(crate) fn set_channel_pressure(&mut self, value: i32) {
        self.channel_pressure = value as u8;
    }

    /// Returns channel pressure as a normalized 0.0-1.0 value.
    pub(crate) fn get_channel_pressure(&self) -> f32 {
        self.channel_pressure as f32 * (1.0 / 127.0)
    }

    pub fn get_channel_pressure_raw(&self) -> u8 {
        self.channel_pressure
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Sends one GS drum instrument NRPN: MSB, then the note number as LSB, then data entry.
    fn drum_nrpn(channel: &mut Channel, msb: i32, note: i32, value: i32) {
        channel.set_nrpn_coarse(msb);
        channel.set_nrpn_fine(note);
        channel.data_entry_coarse(value);
    }

    #[test]
    fn gs_drum_nrpns_are_stored_per_note() {
        let mut channel = Channel::new(true);

        drum_nrpn(&mut channel, 0x18, 38, 66); // pitch coarse, +2 semitones
        drum_nrpn(&mut channel, 0x1A, 38, 64); // level
        drum_nrpn(&mut channel, 0x1C, 38, 127); // pan, hard right
        drum_nrpn(&mut channel, 0x1D, 38, 127); // reverb send
        drum_nrpn(&mut channel, 0x1E, 38, 0); // chorus send

        assert_eq!(channel.get_drum_pitch_coarse(38), 2.0);
        assert_eq!(channel.get_drum_level(38), Some(64.0 / 127.0));
        assert_eq!(channel.get_drum_pan(38), Some(50.0));
        assert_eq!(channel.get_drum_reverb_send(38), Some(1.0));
        assert_eq!(channel.get_drum_chorus_send(38), Some(0.0));

        // Another note keeps the SoundFont values.
        assert_eq!(channel.get_drum_pitch_coarse(36), 0.0);
        assert_eq!(channel.get_drum_level(36), None);
        assert_eq!(channel.get_drum_pan(36), None);

        // A pan of 0 asks for a random position, which is taken as centre.
        drum_nrpn(&mut channel, 0x1C, 40, 0);
        assert_eq!(channel.get_drum_pan(40), Some(0.0));
    }

    #[test]
    fn gs_drum_nrpns_apply_only_to_percussion_channels() {
        let mut channel = Channel::new(false);
        drum_nrpn(&mut channel, 0x1A, 38, 64);
        assert_eq!(channel.get_drum_level(38), None);

        // The value was stored all along and applies once the channel becomes percussion,
        // the same way the bank number follows the flag.
        channel.set_percussion_channel(true);
        assert_eq!(channel.get_drum_level(38), Some(64.0 / 127.0));
    }

    #[test]
    fn gs_drum_nrpns_survive_reset_all_controllers() {
        let mut channel = Channel::new(true);
        drum_nrpn(&mut channel, 0x1A, 38, 64);

        // GS states that a value set by NRPN is not reset by Reset All Controllers.
        channel.reset_all_controllers();
        assert_eq!(channel.get_drum_level(38), Some(64.0 / 127.0));

        channel.reset();
        assert_eq!(channel.get_drum_level(38), None);
    }

    #[test]
    fn percussion_flag_switch_applies_to_current_bank() {
        let mut channel = Channel::new(false);
        channel.set_bank(3);
        assert_eq!(channel.get_bank_number(), 3);

        channel.set_percussion_channel(true);
        assert_eq!(channel.get_bank_number(), 131);

        channel.set_percussion_channel(false);
        assert_eq!(channel.get_bank_number(), 3);
    }

    #[test]
    fn reset_all_controllers_deselects_nrpn() {
        let mut channel = Channel::new(false);
        channel.set_nrpn_coarse(1);
        channel.set_nrpn_fine(8);
        channel.data_entry_coarse(80);
        assert_eq!(channel.get_vibrato_rate_raw(), 80);

        channel.reset_all_controllers();
        channel.data_entry_coarse(20);
        assert_eq!(channel.get_vibrato_rate_raw(), 80);
    }

    #[test]
    fn raw_controller_state_follows_resets() {
        let mut channel = Channel::new(false);
        assert_eq!(channel.get_controller_value(7), 100);
        assert_eq!(channel.get_controller_value(11), 127);
        assert_eq!(channel.get_pitch_bend_raw(), 8192);

        channel.set_controller_value(7, 20);
        channel.set_controller_value(2, 90);
        channel.set_controller_value(11, 30);
        channel.set_controller_value(128, 1);
        channel.set_poly_pressure(60, 77);
        channel.set_poly_pressure(-1, 77);
        channel.set_pitch_bend(0, 0);
        assert_eq!(channel.get_poly_pressure(60), 77);
        assert_eq!(channel.get_pitch_bend_raw(), 0);

        // Reset All Controllers leaves volume and other controllers untouched.
        channel.reset_all_controllers();
        assert_eq!(channel.get_controller_value(7), 20);
        assert_eq!(channel.get_controller_value(2), 90);
        assert_eq!(channel.get_controller_value(11), 127);
        assert_eq!(channel.get_poly_pressure(60), 0);
        assert_eq!(channel.get_pitch_bend_raw(), 8192);

        channel.reset();
        assert_eq!(channel.get_controller_value(7), 100);
        assert_eq!(channel.get_controller_value(2), 0);
    }

    #[test]
    fn reset_keeps_percussion_drum_bank() {
        let mut channel = Channel::new(true);
        channel.set_bank(5);
        channel.reset();
        assert_eq!(channel.get_bank_number(), 128);
    }

    #[test]
    fn xg_mode_uses_bank_lsb_for_melodic_channels() {
        let mut channel = Channel::new(false);
        channel.set_system_mode(SystemMode::Xg);
        channel.set_bank(0);
        channel.set_bank_lsb(5);
        assert_eq!(channel.get_bank_number(), 5);

        channel.set_bank_lsb(0);
        assert_eq!(channel.get_bank_number(), 0);
    }

    #[test]
    fn xg_mode_bank_msb_switches_percussion_flag() {
        let mut channel = Channel::new(false);
        channel.set_system_mode(SystemMode::Xg);
        channel.set_patch(9);

        channel.set_bank(127);
        assert!(channel.get_is_percussion_channel());
        assert_eq!(channel.get_bank_number(), 128);
        assert_eq!(channel.get_patch_number(), 9);

        channel.set_bank(126);
        assert!(channel.get_is_percussion_channel());

        channel.set_bank(0);
        assert!(!channel.get_is_percussion_channel());
        assert_eq!(channel.get_bank_number(), 0);

        channel.set_bank(127);
        channel.set_bank_lsb(3);
        assert_eq!(channel.get_bank_number(), 128);
    }

    #[test]
    fn gm_and_gs_modes_ignore_bank_lsb_and_drum_msb() {
        for mode in [SystemMode::Gm, SystemMode::Gs] {
            let mut channel = Channel::new(false);
            channel.set_system_mode(mode);

            channel.set_bank(0);
            channel.set_bank_lsb(5);
            assert_eq!(channel.get_bank_number(), 0);

            channel.set_bank(127);
            assert!(!channel.get_is_percussion_channel());
            assert_eq!(channel.get_bank_number(), 127);

            channel.set_percussion_channel(true);
            assert_eq!(channel.get_bank_number(), 255);
        }
    }
}
