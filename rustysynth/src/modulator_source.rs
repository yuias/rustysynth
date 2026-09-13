#![allow(dead_code)]

//! Decoding and normalization of SF2.04 modulator source operands (section 8.2.1).

use std::sync::OnceLock;

/// A modulator source operand, as packed into `SFModulator` (SF2.04 8.2.1).
///
/// Bit layout of the wrapped `u16`:
/// - bits 0-6: controller index
/// - bit 7: 0 = general controller palette, 1 = MIDI CC
/// - bit 8: direction, 1 = max-to-min
/// - bit 9: polarity, 1 = bipolar
/// - bits 10-15: curve type (0 linear, 1 concave, 2 convex, 3 switch, 4-63 reserved)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModulatorSource(pub(crate) u16);

/// A resolved controller identity for a modulator source.
///
/// `None` here is the SF2 "no controller" source (general index 0), which is a
/// valid source that always contributes zero; it is distinct from the `Option`
/// returned by `ModulatorSource::controller`, which is `None` for unusable
/// (reserved, linked, or banned) sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Controller {
    None,
    Velocity,
    Key,
    PolyPressure,
    ChannelPressure,
    PitchWheel,
    /// RPN 0 (semitones); fed through `map_7bit` like any other 7-bit value.
    PitchWheelSensitivity,
    Cc(u8),
}

fn is_banned_cc(cc: u8) -> bool {
    matches!(cc, 0 | 6 | 32 | 38 | 98..=101 | 120..=127)
}

fn concave_table() -> &'static [f32; 128] {
    static TABLE: OnceLock<[f32; 128]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0_f32; 128];
        for (i, entry) in table.iter_mut().enumerate().take(127) {
            let x = (127 - i) as f32 / 127.0;
            let db = -(200.0 / 960.0) * (x * x).log10();
            *entry = db.clamp(0.0, 1.0);
        }
        // x -> 0 at i = 127, where log10 diverges; the spec pins this endpoint to 1.0.
        table[127] = 1.0;
        table
    })
}

fn convex_table() -> &'static [f32; 128] {
    static TABLE: OnceLock<[f32; 128]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let concave = concave_table();
        let mut table = [0_f32; 128];
        for (i, entry) in table.iter_mut().enumerate() {
            *entry = 1.0 - concave[127 - i];
        }
        table
    })
}

impl ModulatorSource {
    pub(crate) fn index(self) -> u8 {
        (self.0 & 0x7F) as u8
    }

    pub(crate) fn is_cc(self) -> bool {
        self.0 & 0x80 != 0
    }

    pub(crate) fn direction_max_to_min(self) -> bool {
        self.0 & 0x100 != 0
    }

    pub(crate) fn is_bipolar(self) -> bool {
        self.0 & 0x200 != 0
    }

    pub(crate) fn curve_type(self) -> u8 {
        ((self.0 >> 10) & 0x3F) as u8
    }

    /// Resolves the controller identity, or `None` if the source is unusable:
    /// a reserved general index, a link source (general index 127), a banned
    /// CC, or a reserved curve type.
    pub(crate) fn controller(self) -> Option<Controller> {
        if self.curve_type() > 3 {
            return None;
        }

        if self.is_cc() {
            let cc = self.index();
            if is_banned_cc(cc) {
                None
            } else {
                Some(Controller::Cc(cc))
            }
        } else {
            match self.index() {
                0 => Some(Controller::None),
                2 => Some(Controller::Velocity),
                3 => Some(Controller::Key),
                10 => Some(Controller::PolyPressure),
                13 => Some(Controller::ChannelPressure),
                14 => Some(Controller::PitchWheel),
                16 => Some(Controller::PitchWheelSensitivity),
                _ => None,
            }
        }
    }

    /// True for sources that are only meaningful at note-on (no per-block value).
    pub(crate) fn is_note_on_only(self) -> bool {
        matches!(
            self.controller(),
            Some(Controller::None) | Some(Controller::Velocity) | Some(Controller::Key)
        )
    }

    fn curve_unipolar(self, linear: f32, table_index: usize) -> f32 {
        match self.curve_type() {
            0 => linear,
            1 => concave_table()[table_index],
            2 => convex_table()[table_index],
            3 => {
                if linear < 0.5 {
                    0.0
                } else {
                    1.0
                }
            }
            // Reserved curve types are caught by `controller`; callers that
            // ignore that check get zero contribution rather than a panic.
            _ => 0.0,
        }
    }

    /// Maps a 7-bit controller value (0..=127) to a normalized modulator
    /// value: unipolar sources return 0..1, bipolar sources return -1..1.
    ///
    /// The "no controller" source always maps to 0.0; it is up to the caller
    /// to skip modulators whose primary source is `Controller::None` rather
    /// than apply this zero contribution.
    pub(crate) fn map_7bit(self, value: u8) -> f32 {
        if !self.is_cc() && self.index() == 0 {
            return 0.0;
        }

        let raw = value.min(127);
        let raw = if self.direction_max_to_min() {
            127 - raw
        } else {
            raw
        };

        let linear = raw as f32 / 128.0;
        let y = self.curve_unipolar(linear, raw as usize);

        if self.is_bipolar() {
            2.0 * y - 1.0
        } else {
            y
        }
    }

    /// Maps a 14-bit controller value (0..=16383, as used by the pitch wheel)
    /// to a normalized modulator value, following the same direction/curve/
    /// polarity pipeline as `map_7bit`.
    ///
    /// Concave and convex curves reuse the 128-entry 7-bit tables: the 14-bit
    /// value is reduced to a table index by taking its top 7 bits. This keeps
    /// the mapping table-driven and allocation-free at the cost of losing the
    /// low 7 bits of resolution on non-linear pitch-wheel curves, which the
    /// SF2.04 default modulator set never exercises (the pitch bend default is
    /// linear).
    pub(crate) fn map_14bit(self, value: u16) -> f32 {
        if !self.is_cc() && self.index() == 0 {
            return 0.0;
        }

        let raw = value.min(16383);
        let raw = if self.direction_max_to_min() {
            16383 - raw
        } else {
            raw
        };

        let linear = raw as f32 / 16384.0;
        let table_index = (raw >> 7) as usize;
        let y = self.curve_unipolar(linear, table_index);

        if self.is_bipolar() {
            2.0 * y - 1.0
        } else {
            y
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < EPS,
            "actual={actual} expected={expected}"
        );
    }

    // Index 3 (key, general palette) is used as a neutral base for curve math
    // tests: it is a valid non-"None" source, so `map_7bit`/`map_14bit` never
    // short-circuit to 0.0.
    fn source(curve: u8, direction: bool, bipolar: bool) -> ModulatorSource {
        let mut bits: u16 = 3;
        if direction {
            bits |= 0x100;
        }
        if bipolar {
            bits |= 0x200;
        }
        bits |= (curve as u16) << 10;
        ModulatorSource(bits)
    }

    #[test]
    fn linear_curve_matches_v_over_128() {
        let s = source(0, false, false);
        assert_close(s.map_7bit(0), 0.0);
        assert_close(s.map_7bit(64), 64.0 / 128.0);
        assert_close(s.map_7bit(127), 127.0 / 128.0);
    }

    #[test]
    fn linear_curve_with_direction_inverts_index() {
        let s = source(0, true, false);
        assert_close(s.map_7bit(0), 127.0 / 128.0);
        assert_close(s.map_7bit(64), 63.0 / 128.0);
        assert_close(s.map_7bit(127), 0.0);
    }

    #[test]
    fn linear_curve_bipolar_rescales_to_minus_one_one() {
        let s = source(0, false, true);
        assert_close(s.map_7bit(0), -1.0);
        assert_close(s.map_7bit(64), 2.0 * (64.0 / 128.0) - 1.0);
        assert_close(s.map_7bit(127), 2.0 * (127.0 / 128.0) - 1.0);
    }

    #[test]
    fn concave_curve_matches_hand_computed_values() {
        let s = source(1, false, false);
        assert_close(s.map_7bit(0), 0.0);
        assert_close(s.map_7bit(64), 0.126_860); // -(5/12) * log10(63/127)
        assert_close(s.map_7bit(127), 1.0);
    }

    #[test]
    fn concave_curve_with_direction() {
        let s = source(1, true, false);
        assert_close(s.map_7bit(0), 1.0);
        assert_close(s.map_7bit(64), 0.124_010); // -(5/12) * log10(64/127)
        assert_close(s.map_7bit(127), 0.0);
    }

    #[test]
    fn convex_curve_matches_hand_computed_values() {
        let s = source(2, false, false);
        assert_close(s.map_7bit(0), 0.0);
        assert_close(s.map_7bit(64), 0.875_990); // 1 - concave[63]
        assert_close(s.map_7bit(127), 1.0);
    }

    #[test]
    fn convex_curve_with_direction() {
        let s = source(2, true, false);
        assert_close(s.map_7bit(0), 1.0);
        assert_close(s.map_7bit(64), 0.873_140); // 1 - concave[64]
        assert_close(s.map_7bit(127), 0.0);
    }

    #[test]
    fn convex_is_concave_mirrored() {
        let concave = source(1, false, false);
        let convex = source(2, false, false);
        for i in 0..128u8 {
            assert_close(convex.map_7bit(i), 1.0 - concave.map_7bit(127 - i));
        }
    }

    #[test]
    fn switch_curve_thresholds_at_half() {
        let s = source(3, false, false);
        assert_close(s.map_7bit(0), 0.0);
        assert_close(s.map_7bit(64), 1.0);
        assert_close(s.map_7bit(127), 1.0);

        let sd = source(3, true, false);
        assert_close(sd.map_7bit(0), 1.0);
        assert_close(sd.map_7bit(64), 0.0);
        assert_close(sd.map_7bit(127), 0.0);
    }

    #[test]
    fn switch_curve_bipolar() {
        let s = source(3, false, true);
        assert_close(s.map_7bit(0), -1.0);
        assert_close(s.map_7bit(64), 1.0);
        assert_close(s.map_7bit(127), 1.0);
    }

    #[test]
    fn bipolar_matches_2y_minus_1_for_every_curve_and_direction() {
        // Fills out the curve x polarity x direction grid: bipolar output is
        // always a rescale of the unipolar output already checked above.
        for curve in 0..=3u8 {
            for direction in [false, true] {
                let unipolar = source(curve, direction, false);
                let bipolar = source(curve, direction, true);
                for v in [0u8, 64, 127] {
                    assert_close(bipolar.map_7bit(v), 2.0 * unipolar.map_7bit(v) - 1.0);
                }
            }
        }
    }

    #[test]
    fn concave_matches_default_velocity_attenuation_formula() {
        // Default modulator #1 (SF2.04 8.4.1): velocity -> attenuation,
        // concave curve, direction max-to-min. At amount 960 centibels this
        // reduces to the classic 40*log10(v/127) dB velocity curve.
        let s = ModulatorSource(0x0502);
        for v in 1..=127u8 {
            let y = s.map_7bit(v);
            let actual_db = -960.0 * y / 10.0;
            let expected_db = 40.0 * (v as f32 / 127.0).log10();
            assert!(
                (actual_db - expected_db).abs() < 1e-3,
                "v={v} actual={actual_db} expected={expected_db}"
            );
        }
    }

    #[test]
    fn pitch_wheel_centre_maps_to_zero_and_half() {
        // Default modulator #10 (SF2.04 8.4.10): pitch wheel, linear, bipolar.
        let bipolar = ModulatorSource(0x020E);
        assert_close(bipolar.map_14bit(8192), 0.0);

        let unipolar = ModulatorSource(0x020E & !0x0200);
        assert_close(unipolar.map_14bit(8192), 0.5);
    }

    #[test]
    fn none_controller_maps_to_zero() {
        let s = ModulatorSource(0);
        assert_close(s.map_7bit(127), 0.0);
        assert_close(s.map_14bit(16383), 0.0);
    }

    #[test]
    fn controller_accepts_valid_general_and_cc_sources() {
        assert_eq!(ModulatorSource(0).controller(), Some(Controller::None));
        assert_eq!(ModulatorSource(2).controller(), Some(Controller::Velocity));
        assert_eq!(ModulatorSource(3).controller(), Some(Controller::Key));
        assert_eq!(
            ModulatorSource(10).controller(),
            Some(Controller::PolyPressure)
        );
        assert_eq!(
            ModulatorSource(13).controller(),
            Some(Controller::ChannelPressure)
        );
        assert_eq!(
            ModulatorSource(14).controller(),
            Some(Controller::PitchWheel)
        );
        assert_eq!(
            ModulatorSource(16).controller(),
            Some(Controller::PitchWheelSensitivity)
        );
        assert_eq!(
            ModulatorSource(0x80 | 1).controller(),
            Some(Controller::Cc(1))
        );
    }

    #[test]
    fn controller_rejects_reserved_general_indices() {
        assert_eq!(ModulatorSource(1).controller(), None);
        assert_eq!(ModulatorSource(4).controller(), None);
        assert_eq!(ModulatorSource(127).controller(), None); // link source
    }

    #[test]
    fn controller_rejects_banned_cc() {
        for cc in [
            0u8, 6, 32, 38, 98, 99, 100, 101, 120, 121, 122, 123, 124, 125, 126, 127,
        ] {
            let s = ModulatorSource(0x80 | cc as u16);
            assert_eq!(s.controller(), None, "cc {cc} should be banned");
        }
    }

    #[test]
    fn controller_rejects_reserved_curve_type() {
        let s = ModulatorSource(2 | (4 << 10));
        assert_eq!(s.controller(), None);
    }

    #[test]
    fn is_note_on_only_matches_spec() {
        assert!(ModulatorSource(0).is_note_on_only());
        assert!(ModulatorSource(2).is_note_on_only());
        assert!(ModulatorSource(3).is_note_on_only());
        assert!(!ModulatorSource(14).is_note_on_only());
        assert!(!ModulatorSource(0x80 | 1).is_note_on_only());
    }
}
