#![allow(dead_code)]

use std::io::Read;

use crate::binary_reader::BinaryReader;
use crate::error::SoundFontError;
use crate::generator_type::GeneratorType;

/// Represents a single modulator record (SF2.04 PMOD/IMOD, section 8.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Modulator {
    pub(crate) source: u16,
    pub(crate) destination: u16,
    pub(crate) amount: i16,
    pub(crate) amount_source: u16,
    pub(crate) transform: u16,
}

/// Whether a modulator list belongs to an instrument or a preset. Preset regions
/// ban extra destinations because preset generators are relative offsets, not
/// absolute values.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ModulatorLevel {
    Instrument,
    Preset,
}

/// Destinations meaningless to modulate at either level: an id/link (INSTRUMENT,
/// SAMPLE_ID) or a range gate (KEY_RANGE, VELOCITY_RANGE).
const COMMON_INVALID_DESTINATIONS: [u16; 4] = [
    GeneratorType::INSTRUMENT,
    GeneratorType::KEY_RANGE,
    GeneratorType::VELOCITY_RANGE,
    GeneratorType::SAMPLE_ID,
];

/// Destinations only meaningful as absolute (instrument-level) values, so a
/// preset region (whose generators are relative offsets) must not target them.
const PRESET_EXTRA_INVALID_DESTINATIONS: [u16; 13] = [
    GeneratorType::START_ADDRESS_OFFSET,
    GeneratorType::END_ADDRESS_OFFSET,
    GeneratorType::START_LOOP_ADDRESS_OFFSET,
    GeneratorType::END_LOOP_ADDRESS_OFFSET,
    GeneratorType::START_ADDRESS_COARSE_OFFSET,
    GeneratorType::END_ADDRESS_COARSE_OFFSET,
    GeneratorType::START_LOOP_ADDRESS_COARSE_OFFSET,
    GeneratorType::KEY_NUMBER,
    GeneratorType::VELOCITY,
    GeneratorType::END_LOOP_ADDRESS_COARSE_OFFSET,
    GeneratorType::SAMPLE_MODES,
    GeneratorType::EXCLUSIVE_CLASS,
    GeneratorType::OVERRIDING_ROOT_KEY,
];

/// Why a modulator was dropped during region-level validation (SF2.04 8.2.1 for
/// source operators, 8.2.9/8.2.10 for transform/destination).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ModulatorDropReason {
    ReservedSource,
    PrimarySourceNone,
    Linked,
    /// The amount source is a link, which the specification never allows.
    LinkedAmountSource,
    /// A link destination naming a modulator index the zone does not have.
    DanglingLink,
    /// A link source that no modulator in the zone feeds.
    UnfedLinkSource,
    BannedCc,
    InvalidTransform,
    InvalidDestination,
}

/// How a source-operator field (`source` or `amount_source`) decodes per
/// SF2.04 8.2.1.
enum SourceClass {
    None,
    Valid,
    Linked,
    Reserved,
    BannedCc,
}

fn classify_source(value: u16) -> SourceClass {
    let index = value & 0x007F;
    let cc_flag = value & 0x0080 != 0;
    let curve_type = (value >> 10) & 0x003F;

    if curve_type > 3 {
        return SourceClass::Reserved;
    }

    if cc_flag {
        match index {
            0 | 6 | 32 | 38 | 98..=101 | 120..=127 => SourceClass::BannedCc,
            _ => SourceClass::Valid,
        }
    } else {
        match index {
            0 => SourceClass::None,
            2 | 3 | 10 | 13 | 14 | 16 => SourceClass::Valid,
            127 => SourceClass::Linked,
            _ => SourceClass::Reserved,
        }
    }
}

impl Modulator {
    fn new<R: Read>(reader: &mut R) -> Result<Self, SoundFontError> {
        let source = BinaryReader::read_u16(reader)?;
        let destination = BinaryReader::read_u16(reader)?;
        let amount = BinaryReader::read_i16(reader)?;
        let amount_source = BinaryReader::read_u16(reader)?;
        let transform = BinaryReader::read_u16(reader)?;

        Ok(Self {
            source,
            destination,
            amount,
            amount_source,
            transform,
        })
    }

    /// Reads a PMOD/IMOD sub-chunk. A chunk size that isn't a positive multiple
    /// of 10 bytes (one modulator record) is malformed; rather than failing the
    /// whole load, the chunk's bytes are discarded, a warning is recorded, and
    /// no modulators are returned for it.
    pub(crate) fn read_from_chunk<R: Read>(
        reader: &mut R,
        size: usize,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<Modulator>, SoundFontError> {
        if size == 0 || size % 10 != 0 {
            BinaryReader::discard_data(reader, size)?;
            warnings.push(format!(
                "a modulator chunk had an invalid size ({} bytes, not a positive multiple of 10); its modulators were ignored",
                size
            ));
            return Ok(Vec::new());
        }

        let count = size / 10 - 1;

        let mut modulators: Vec<Modulator> = Vec::new();
        for _i in 0..count {
            modulators.push(Modulator::new(reader)?);
        }

        // The last one is the terminator.
        Modulator::new(reader)?;

        Ok(modulators)
    }

    /// Identity used for zone-local dedup and global/local merge: everything but
    /// the amount, which is the one field allowed to differ between "the same"
    /// modulator at different scopes.
    pub(crate) fn same_identity(&self, other: &Modulator) -> bool {
        self.source == other.source
            && self.destination == other.destination
            && self.amount_source == other.amount_source
            && self.transform == other.transform
    }

    /// True when the destination names another modulator in the same zone rather than a
    /// generator. The remaining 15 bits are that modulator's index, counted from the first
    /// modulator of the zone.
    fn is_link_destination(&self) -> bool {
        self.destination & 0x8000 != 0
    }

    fn link_target(&self) -> usize {
        (self.destination & 0x7FFF) as usize
    }

    fn has_link_source(&self) -> bool {
        matches!(classify_source(self.source), SourceClass::Linked)
    }

    /// Checks whether the modulator is usable at all, per SF2.04 8.2.1 (source
    /// operator encoding) and 8.2.9/8.2.10 (transform, destination).
    fn validate(&self, level: ModulatorLevel) -> Result<(), ModulatorDropReason> {
        match classify_source(self.source) {
            SourceClass::None => return Err(ModulatorDropReason::PrimarySourceNone),
            // A link source is only meaningful with the rest of the zone in view, so
            // `validate_zone` decides its fate before calling this.
            SourceClass::Linked => return Err(ModulatorDropReason::Linked),
            SourceClass::Reserved => return Err(ModulatorDropReason::ReservedSource),
            SourceClass::BannedCc => return Err(ModulatorDropReason::BannedCc),
            SourceClass::Valid => {}
        }

        // Unlike the primary source, "None" is a legitimate amount source
        // (a fixed amount with no controller scaling it).
        match classify_source(self.amount_source) {
            SourceClass::Linked => return Err(ModulatorDropReason::LinkedAmountSource),
            SourceClass::Reserved => return Err(ModulatorDropReason::ReservedSource),
            SourceClass::BannedCc => return Err(ModulatorDropReason::BannedCc),
            SourceClass::None | SourceClass::Valid => {}
        }

        if self.transform != 0 && self.transform != 2 {
            return Err(ModulatorDropReason::InvalidTransform);
        }

        if self.is_link_destination() {
            return Err(ModulatorDropReason::Linked);
        }
        let is_out_of_range = self.destination as usize >= GeneratorType::COUNT;
        let is_common_invalid = COMMON_INVALID_DESTINATIONS.contains(&self.destination);
        let is_preset_invalid = level == ModulatorLevel::Preset
            && PRESET_EXTRA_INVALID_DESTINATIONS.contains(&self.destination);
        if is_out_of_range || is_common_invalid || is_preset_invalid {
            return Err(ModulatorDropReason::InvalidDestination);
        }

        Ok(())
    }
}

/// Validates a zone's modulators and collapses same-identity duplicates in
/// place, keeping the last occurrence's amount.
///
/// FluidSynth's handling of zone-local duplicates is unverified against this
/// "last wins" choice.
pub(crate) fn validate_zone(
    zone_modulators: &[Modulator],
    level: ModulatorLevel,
    counts: &mut ModulatorDropCounts,
) -> Vec<Modulator> {
    // Link indices are relative to the first modulator of the zone, so whether a link is
    // usable can only be decided here. Chains are not evaluated, but the reason a linked
    // modulator was dropped is worth telling apart: a dangling or unfed link is invalid by
    // the specification, while a well-formed chain is this synthesizer's own limitation.
    let mut is_link_target = vec![false; zone_modulators.len()];
    for modulator in zone_modulators {
        if modulator.is_link_destination() && modulator.link_target() < zone_modulators.len() {
            is_link_target[modulator.link_target()] = true;
        }
    }

    let mut result: Vec<Modulator> = Vec::new();
    for (index, modulator) in zone_modulators.iter().enumerate() {
        if modulator.is_link_destination() && modulator.link_target() >= zone_modulators.len() {
            counts.record(ModulatorDropReason::DanglingLink);
            continue;
        }
        if modulator.has_link_source() && !is_link_target[index] {
            counts.record(ModulatorDropReason::UnfedLinkSource);
            continue;
        }
        match modulator.validate(level) {
            Ok(()) => {
                if let Some(existing) = result.iter_mut().find(|m| m.same_identity(modulator)) {
                    *existing = *modulator;
                } else {
                    result.push(*modulator);
                }
            }
            Err(reason) => counts.record(reason),
        }
    }
    result
}

/// Merges a region's global and local modulator lists, with local entries
/// replacing identical global ones. Both lists are expected to already be
/// validated and zone-locally deduped (see `validate_zone`).
pub(crate) fn merge(global: &[Modulator], local: Vec<Modulator>) -> Box<[Modulator]> {
    let mut merged = local;
    for modulator in global {
        if !merged.iter().any(|m| m.same_identity(modulator)) {
            merged.push(*modulator);
        }
    }
    merged.into_boxed_slice()
}

/// Aggregates modulator drop reasons across an entire SoundFont so loading
/// emits one summary line per reason instead of one line per modulator.
#[derive(Default)]
pub(crate) struct ModulatorDropCounts {
    reserved_source: usize,
    primary_source_none: usize,
    linked: usize,
    linked_amount_source: usize,
    dangling_link: usize,
    unfed_link_source: usize,
    banned_cc: usize,
    invalid_transform: usize,
    invalid_destination: usize,
}

impl ModulatorDropCounts {
    fn record(&mut self, reason: ModulatorDropReason) {
        match reason {
            ModulatorDropReason::ReservedSource => self.reserved_source += 1,
            ModulatorDropReason::PrimarySourceNone => self.primary_source_none += 1,
            ModulatorDropReason::Linked => self.linked += 1,
            ModulatorDropReason::LinkedAmountSource => self.linked_amount_source += 1,
            ModulatorDropReason::DanglingLink => self.dangling_link += 1,
            ModulatorDropReason::UnfedLinkSource => self.unfed_link_source += 1,
            ModulatorDropReason::BannedCc => self.banned_cc += 1,
            ModulatorDropReason::InvalidTransform => self.invalid_transform += 1,
            ModulatorDropReason::InvalidDestination => self.invalid_destination += 1,
        }
    }

    /// Converts the accumulated counts into human-readable warning lines, one
    /// per nonzero reason.
    pub(crate) fn into_messages(self) -> Vec<String> {
        let mut messages = Vec::new();

        if self.reserved_source > 0 {
            messages.push(format!(
                "{} modulator(s) dropped: reserved source controller index or curve type",
                self.reserved_source
            ));
        }
        if self.primary_source_none > 0 {
            messages.push(format!(
                "{} modulator(s) dropped: primary source is None",
                self.primary_source_none
            ));
        }
        if self.linked > 0 {
            messages.push(format!(
                "{} modulator(s) dropped: modulator chains are not supported",
                self.linked
            ));
        }
        if self.linked_amount_source > 0 {
            messages.push(format!(
                "{} modulator(s) dropped: the amount source cannot be a link",
                self.linked_amount_source
            ));
        }
        if self.dangling_link > 0 {
            messages.push(format!(
                "{} modulator(s) dropped: the link points past the end of the zone",
                self.dangling_link
            ));
        }
        if self.unfed_link_source > 0 {
            messages.push(format!(
                "{} modulator(s) dropped: the link source has no modulator feeding it",
                self.unfed_link_source
            ));
        }
        if self.banned_cc > 0 {
            messages.push(format!(
                "{} modulator(s) dropped: banned MIDI CC used as a source",
                self.banned_cc
            ));
        }
        if self.invalid_transform > 0 {
            messages.push(format!(
                "{} modulator(s) dropped: unsupported transform",
                self.invalid_transform
            ));
        }
        if self.invalid_destination > 0 {
            messages.push(format!(
                "{} modulator(s) dropped: invalid or non-modulatable destination",
                self.invalid_destination
            ));
        }

        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::File;
    use std::path::{Path, PathBuf};

    use crate::read_counter::ReadCounter;
    use crate::soundfont_info::SoundFontInfo;
    use crate::soundfont_sampledata::SoundFontSampleData;

    fn modulator(source: u16, destination: u16, amount: i16, amount_source: u16, transform: u16) -> Modulator {
        Modulator {
            source,
            destination,
            amount,
            amount_source,
            transform,
        }
    }

    // CC1 (modulation wheel) -> vibrato LFO to pitch, a plausible real modulator.
    fn valid_modulator(amount: i16) -> Modulator {
        modulator(0x0081, GeneratorType::VIBRATO_LFO_TO_PITCH, amount, 0, 0)
    }

    #[test]
    fn valid_modulator_passes_validation() {
        assert!(valid_modulator(50).validate(ModulatorLevel::Instrument).is_ok());
        assert!(valid_modulator(50).validate(ModulatorLevel::Preset).is_ok());
    }

    #[test]
    fn reserved_source_curve_type_is_dropped() {
        // Curve type bits (10-15) set to 4, which is reserved.
        let m = modulator(0x1002, GeneratorType::PAN, 100, 0, 0);
        assert_eq!(
            m.validate(ModulatorLevel::Instrument),
            Err(ModulatorDropReason::ReservedSource)
        );
    }

    #[test]
    fn primary_source_none_is_dropped() {
        let m = modulator(0, GeneratorType::PAN, 100, 0, 0);
        assert_eq!(
            m.validate(ModulatorLevel::Instrument),
            Err(ModulatorDropReason::PrimarySourceNone)
        );
    }

    #[test]
    fn amount_source_none_is_allowed() {
        let m = modulator(0x0002, GeneratorType::PAN, 100, 0, 0);
        assert!(m.validate(ModulatorLevel::Instrument).is_ok());
    }

    #[test]
    fn zone_validation_tells_the_link_failures_apart() {
        const LINK_SOURCE: u16 = 127;

        // A well-formed pair: modulator 0 feeds modulator 1, which drives a generator.
        // Both are dropped, but only because chains are unsupported.
        let chain = [
            modulator(0x0081, 0x8000 | 1, 100, 0, 0),
            modulator(LINK_SOURCE, GeneratorType::VIBRATO_LFO_TO_PITCH, 50, 0, 0),
        ];
        let mut counts = ModulatorDropCounts::default();
        assert!(validate_zone(&chain, ModulatorLevel::Instrument, &mut counts).is_empty());
        assert_eq!(counts.linked, 2);
        assert_eq!(counts.dangling_link, 0);
        assert_eq!(counts.unfed_link_source, 0);

        // A link naming a modulator the zone does not have.
        let dangling = [modulator(0x0081, 0x8000 | 7, 100, 0, 0)];
        let mut counts = ModulatorDropCounts::default();
        assert!(validate_zone(&dangling, ModulatorLevel::Instrument, &mut counts).is_empty());
        assert_eq!(counts.dangling_link, 1);
        assert_eq!(counts.linked, 0);

        // A link source with nothing feeding it.
        let unfed = [
            modulator(LINK_SOURCE, GeneratorType::VIBRATO_LFO_TO_PITCH, 50, 0, 0),
            valid_modulator(50),
        ];
        let mut counts = ModulatorDropCounts::default();
        let kept = validate_zone(&unfed, ModulatorLevel::Instrument, &mut counts);
        assert_eq!(kept.len(), 1);
        assert_eq!(counts.unfed_link_source, 1);
        assert_eq!(counts.linked, 0);

        // A link used as the amount source is never legal.
        let amount_link = [modulator(
            0x0081,
            GeneratorType::VIBRATO_LFO_TO_PITCH,
            50,
            LINK_SOURCE,
            0,
        )];
        let mut counts = ModulatorDropCounts::default();
        assert!(validate_zone(&amount_link, ModulatorLevel::Instrument, &mut counts).is_empty());
        assert_eq!(counts.linked_amount_source, 1);
        assert_eq!(counts.linked, 0);
    }

    #[test]
    fn linked_source_is_dropped() {
        // General controller index 127 (link), CC flag clear.
        let m = modulator(0x007F, GeneratorType::PAN, 100, 0, 0);
        assert_eq!(m.validate(ModulatorLevel::Instrument), Err(ModulatorDropReason::Linked));
    }

    #[test]
    fn linked_destination_is_dropped() {
        let m = modulator(0x0002, GeneratorType::PAN | 0x8000, 100, 0, 0);
        assert_eq!(m.validate(ModulatorLevel::Instrument), Err(ModulatorDropReason::Linked));
    }

    #[test]
    fn banned_cc6_source_is_dropped() {
        // CC flag set (bit 7), controller number 6 (data entry MSB), banned.
        let m = modulator(0x0086, GeneratorType::PAN, 100, 0, 0);
        assert_eq!(m.validate(ModulatorLevel::Instrument), Err(ModulatorDropReason::BannedCc));
    }

    #[test]
    fn banned_cc120_source_is_dropped() {
        // CC flag set, controller number 120 (all sound off), banned.
        let m = modulator(0x00F8, GeneratorType::PAN, 100, 0, 0);
        assert_eq!(m.validate(ModulatorLevel::Instrument), Err(ModulatorDropReason::BannedCc));
    }

    #[test]
    fn transform_one_is_dropped() {
        let m = modulator(0x0002, GeneratorType::PAN, 100, 0, 1);
        assert_eq!(
            m.validate(ModulatorLevel::Instrument),
            Err(ModulatorDropReason::InvalidTransform)
        );
    }

    #[test]
    fn sample_modes_destination_is_valid_for_instrument_but_not_preset() {
        let m = modulator(0x0002, GeneratorType::SAMPLE_MODES, 1, 0, 0);
        assert!(m.validate(ModulatorLevel::Instrument).is_ok());
        assert_eq!(
            m.validate(ModulatorLevel::Preset),
            Err(ModulatorDropReason::InvalidDestination)
        );
    }

    #[test]
    fn common_invalid_destination_is_dropped_at_both_levels() {
        let m = modulator(0x0002, GeneratorType::SAMPLE_ID, 1, 0, 0);
        assert_eq!(
            m.validate(ModulatorLevel::Instrument),
            Err(ModulatorDropReason::InvalidDestination)
        );
        assert_eq!(
            m.validate(ModulatorLevel::Preset),
            Err(ModulatorDropReason::InvalidDestination)
        );
    }

    #[test]
    fn later_duplicate_within_zone_wins() {
        let mut counts = ModulatorDropCounts::default();
        let zone = vec![valid_modulator(50), valid_modulator(75)];
        let result = validate_zone(&zone, ModulatorLevel::Instrument, &mut counts);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].amount, 75);
        // A same-zone override is not a validation failure.
        assert_eq!(counts.into_messages().len(), 0);
    }

    #[test]
    fn invalid_modulators_are_dropped_and_counted() {
        let mut counts = ModulatorDropCounts::default();
        let zone = vec![
            valid_modulator(50),
            modulator(0, GeneratorType::PAN, 1, 0, 0), // primary source None
            modulator(0x0086, GeneratorType::PAN, 1, 0, 0), // banned CC6
        ];
        let result = validate_zone(&zone, ModulatorLevel::Instrument, &mut counts);
        assert_eq!(result.len(), 1);
        assert_eq!(counts.into_messages().len(), 2);
    }

    #[test]
    fn local_replaces_identical_global() {
        let global = vec![valid_modulator(50)];
        let mut counts = ModulatorDropCounts::default();
        let global = validate_zone(&global, ModulatorLevel::Instrument, &mut counts);
        let local = validate_zone(&[valid_modulator(75)], ModulatorLevel::Instrument, &mut counts);

        let merged = merge(&global, local);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].amount, 75);
    }

    #[test]
    fn merge_keeps_non_overridden_global_entries() {
        let global_source = vec![valid_modulator(50), modulator(0x028A, GeneratorType::PAN, 1000, 0, 0)];
        let mut counts = ModulatorDropCounts::default();
        let global = validate_zone(&global_source, ModulatorLevel::Instrument, &mut counts);
        let local = validate_zone(&[valid_modulator(75)], ModulatorLevel::Instrument, &mut counts);

        let merged = merge(&global, local);

        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|m| m.destination == GeneratorType::PAN && m.amount == 1000));
        assert!(merged
            .iter()
            .any(|m| m.destination == GeneratorType::VIBRATO_LFO_TO_PITCH && m.amount == 75));
    }

    /// Repository-root SoundFont files are untracked; skip rather than fail
    /// when they aren't present on disk.
    fn repo_root_file(name: &str) -> Option<PathBuf> {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.pop();
        path.push(name);
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    /// Counts raw pmod/imod records (terminal excluded), independent of the
    /// region-level validation/merge that `SoundFont::new` applies. Mirrors
    /// `SoundFontParameters::new`'s chunk walk up through pdta, since only
    /// the pmod/imod sizes are of interest here.
    fn raw_modulator_counts(path: &Path) -> (usize, usize) {
        let mut file = File::open(path).unwrap();

        BinaryReader::read_four_cc(&mut file).unwrap(); // "RIFF"
        BinaryReader::read_u32(&mut file).unwrap(); // overall size
        BinaryReader::read_four_cc(&mut file).unwrap(); // "sfbk"

        SoundFontInfo::new(&mut file).unwrap();
        SoundFontSampleData::new(&mut file).unwrap();

        let chunk_id = BinaryReader::read_four_cc(&mut file).unwrap();
        assert_eq!(chunk_id.as_bytes(), b"LIST");
        let end = BinaryReader::read_u32(&mut file).unwrap() as usize;
        let reader = &mut ReadCounter::new(&mut file);
        let list_type = BinaryReader::read_four_cc(reader).unwrap();
        assert_eq!(list_type.as_bytes(), b"pdta");

        let mut warnings: Vec<String> = Vec::new();
        let mut imod_count: Option<usize> = None;
        let mut pmod_count: Option<usize> = None;

        while reader.bytes_read() < end {
            let id = BinaryReader::read_four_cc(reader).unwrap();
            let size = BinaryReader::read_u32(reader).unwrap() as usize;

            match id.as_bytes() {
                b"pmod" => {
                    pmod_count = Some(
                        Modulator::read_from_chunk(reader, size, &mut warnings)
                            .unwrap()
                            .len(),
                    );
                }
                b"imod" => {
                    imod_count = Some(
                        Modulator::read_from_chunk(reader, size, &mut warnings)
                            .unwrap()
                            .len(),
                    );
                }
                _ => BinaryReader::discard_data(reader, size).unwrap(),
            }
        }

        (imod_count.unwrap(), pmod_count.unwrap())
    }

    #[test]
    fn general_user_gs_musescore_raw_modulator_counts() {
        let Some(path) = repo_root_file("GeneralUser GS MuseScore v1.442.sf2") else {
            return;
        };
        let (imod, pmod) = raw_modulator_counts(&path);
        assert_eq!(imod, 2151);
        assert_eq!(pmod, 363);
    }

    #[test]
    fn timgm6mb_raw_modulator_counts() {
        let Some(path) = repo_root_file("TimGM6mb.sf2") else {
            return;
        };
        let (imod, pmod) = raw_modulator_counts(&path);
        assert_eq!(imod, 455);
        assert_eq!(pmod, 0);
    }
}
