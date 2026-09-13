//! Builds a minimal, valid SoundFont 2 file in memory so unit tests elsewhere in the
//! crate can construct a `Synthesizer` without the full-size `.sf2` files, which are
//! not tracked in the repository.
//!
//! Only the handful of fields the parser actually reads are populated; everything
//! else uses spec-mandated defaults (e.g. terminal records for phdr/inst/shdr).

use std::io::Cursor;
use std::sync::Arc;

use crate::generator_type::GeneratorType;
use crate::soundfont::SoundFont;

/// Zero samples the SF2 spec requires after each sample's data in `smpl`, so that
/// interpolation reads past a region's nominal end land on silence instead of the
/// next sample's audio.
const SAMPLE_PADDING: usize = 46;

struct SampleHeaderRecord {
    name: String,
    start: i32,
    end: i32,
    start_loop: i32,
    end_loop: i32,
    sample_rate: i32,
    original_pitch: u8,
}

struct InstrumentInfoRecord {
    name: String,
    zone_start_index: u16,
}

struct PresetInfoRecord {
    name: String,
    bank: i32,
    patch: i32,
    zone_start_index: u16,
}

/// A generator list for a single zone, paired with the index of the resource
/// (sample, for an instrument zone; instrument, for a preset zone) the zone binds to.
/// The id generator (sampleID / instrument) is appended by the builder, not the caller.
type ZoneSpec = (Vec<(u16, i16)>, usize);

/// Incrementally assembles a well-formed SF2 file in memory.
///
/// Usage: add samples, then instruments (referencing sample indices), then presets
/// (referencing instrument indices), then call `build()` to get the RIFF bytes.
pub(crate) struct SoundFontBuilder {
    smpl: Vec<i16>,
    sample_headers: Vec<SampleHeaderRecord>,

    ibag: Vec<(u16, u16)>,
    igen: Vec<(u16, i16)>,
    instruments: Vec<InstrumentInfoRecord>,

    pbag: Vec<(u16, u16)>,
    pgen: Vec<(u16, i16)>,
    presets: Vec<PresetInfoRecord>,
}

impl SoundFontBuilder {
    pub(crate) fn new() -> Self {
        Self {
            smpl: Vec::new(),
            sample_headers: Vec::new(),
            ibag: Vec::new(),
            igen: Vec::new(),
            instruments: Vec::new(),
            pbag: Vec::new(),
            pgen: Vec::new(),
            presets: Vec::new(),
        }
    }

    /// Appends a sample to `smpl` and its header to `shdr`.
    ///
    /// `loop_start`/`loop_end` are offsets relative to the start of `data`; the stored
    /// header fields are absolute positions in the final `smpl` chunk.
    ///
    /// Returns the sample index for use as the second element of an instrument zone spec.
    pub(crate) fn sample(
        &mut self,
        name: &str,
        data: &[i16],
        sample_rate: i32,
        original_pitch: u8,
        loop_start: i32,
        loop_end: i32,
    ) -> usize {
        let start = self.smpl.len() as i32;
        self.smpl.extend_from_slice(data);
        let end = self.smpl.len() as i32;
        self.smpl.extend(std::iter::repeat(0_i16).take(SAMPLE_PADDING));

        self.sample_headers.push(SampleHeaderRecord {
            name: name.to_string(),
            start,
            end,
            start_loop: start + loop_start,
            end_loop: start + loop_end,
            sample_rate,
            original_pitch,
        });

        self.sample_headers.len() - 1
    }

    /// Adds an instrument made of the given zones. Each zone's generator list must not
    /// include the sampleID generator; the builder appends it from the zone's sample index.
    ///
    /// Returns the instrument index for use in a preset zone spec.
    pub(crate) fn instrument(&mut self, name: &str, zones: Vec<ZoneSpec>) -> usize {
        let zone_start_index = self.ibag.len() as u16;

        for (generators, sample_index) in zones {
            let gen_index = self.igen.len() as u16;
            self.ibag.push((gen_index, 0));
            self.igen.extend(Self::ordered_generators(
                generators,
                (GeneratorType::SAMPLE_ID, sample_index as i16),
            ));
        }

        self.instruments.push(InstrumentInfoRecord {
            name: name.to_string(),
            zone_start_index,
        });

        self.instruments.len() - 1
    }

    /// Adds a preset made of the given zones. Each zone's generator list must not
    /// include the instrument generator; the builder appends it from the zone's
    /// instrument index.
    pub(crate) fn preset(
        &mut self,
        name: &str,
        bank: i32,
        patch: i32,
        zones: Vec<ZoneSpec>,
    ) -> usize {
        let zone_start_index = self.pbag.len() as u16;

        for (generators, instrument_index) in zones {
            let gen_index = self.pgen.len() as u16;
            self.pbag.push((gen_index, 0));
            self.pgen.extend(Self::ordered_generators(
                generators,
                (GeneratorType::INSTRUMENT, instrument_index as i16),
            ));
        }

        self.presets.push(PresetInfoRecord {
            name: name.to_string(),
            bank,
            patch,
            zone_start_index,
        });

        self.presets.len() - 1
    }

    /// Orders a zone's generators per spec: key range and velocity range (when present)
    /// come first, then the rest in caller-provided order, then the id generator last.
    fn ordered_generators(mut generators: Vec<(u16, i16)>, terminal: (u16, i16)) -> Vec<(u16, i16)> {
        let mut ordered = Vec::with_capacity(generators.len() + 1);
        for range_generator in [GeneratorType::KEY_RANGE, GeneratorType::VELOCITY_RANGE] {
            if let Some(pos) = generators.iter().position(|(t, _)| *t == range_generator) {
                ordered.push(generators.remove(pos));
            }
        }
        ordered.extend(generators);
        ordered.push(terminal);
        ordered
    }

    /// Serializes the accumulated samples/instruments/presets into a complete
    /// RIFF/sfbk SoundFont 2 file.
    pub(crate) fn build(mut self) -> Vec<u8> {
        // Terminal ibag/igen records, and the "EOI" instrument info record.
        let instrument_zone_count = self.ibag.len() as u16;
        let ibag_terminal_gen_index = self.igen.len() as u16;
        self.ibag.push((ibag_terminal_gen_index, 0));
        self.igen.push((0, 0));
        self.instruments.push(InstrumentInfoRecord {
            name: "EOI".to_string(),
            zone_start_index: instrument_zone_count,
        });

        // Terminal pbag/pgen records, and the "EOP" preset info record.
        let preset_zone_count = self.pbag.len() as u16;
        let pbag_terminal_gen_index = self.pgen.len() as u16;
        self.pbag.push((pbag_terminal_gen_index, 0));
        self.pgen.push((0, 0));
        self.presets.push(PresetInfoRecord {
            name: "EOP".to_string(),
            bank: 0,
            patch: 0,
            zone_start_index: preset_zone_count,
        });

        // Terminal sample header ("EOS").
        self.sample_headers.push(SampleHeaderRecord {
            name: "EOS".to_string(),
            start: 0,
            end: 0,
            start_loop: 0,
            end_loop: 0,
            sample_rate: 0,
            original_pitch: 0,
        });

        let info = list_chunk(b"INFO", vec![info_ifil(), info_text(b"INAM", "Test SoundFont")]);

        let sdta = list_chunk(b"sdta", vec![chunk(b"smpl", wave_bytes(&self.smpl))]);

        let pdta = list_chunk(
            b"pdta",
            vec![
                chunk(b"phdr", phdr_bytes(&self.presets)),
                chunk(b"pbag", bag_bytes(&self.pbag)),
                chunk(b"pmod", terminal_modulator()),
                chunk(b"pgen", gen_bytes(&self.pgen)),
                chunk(b"inst", inst_bytes(&self.instruments)),
                chunk(b"ibag", bag_bytes(&self.ibag)),
                chunk(b"imod", terminal_modulator()),
                chunk(b"igen", gen_bytes(&self.igen)),
                chunk(b"shdr", shdr_bytes(&self.sample_headers)),
            ],
        );

        let mut riff_data = Vec::new();
        riff_data.extend_from_slice(b"sfbk");
        riff_data.extend_from_slice(&info);
        riff_data.extend_from_slice(&sdta);
        riff_data.extend_from_slice(&pdta);

        chunk(b"RIFF", riff_data)
    }
}

// --- Byte-level helpers -----------------------------------------------------

/// Writes a RIFF chunk (id + size + data).
///
/// The parser reads sub-chunks of a LIST back-to-back without ever skipping a RIFF
/// pad byte, so every sub-chunk written here must have an even-sized payload; the pad
/// byte below only guards the (currently unreachable) case of an odd-sized payload
/// slipping in, and must never be relied upon for anything but the outermost chunk.
fn chunk(id: &[u8; 4], data: Vec<u8>) -> Vec<u8> {
    let len = data.len();
    let mut out = Vec::with_capacity(8 + len + 1);
    out.extend_from_slice(id);
    out.extend_from_slice(&(len as u32).to_le_bytes());
    out.extend_from_slice(&data);
    if len % 2 != 0 {
        out.push(0);
    }
    out
}

fn list_chunk(list_type: &[u8; 4], subchunks: Vec<Vec<u8>>) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(list_type);
    for subchunk in subchunks {
        data.extend_from_slice(&subchunk);
    }
    chunk(b"LIST", data)
}

fn push_fixed_str(buf: &mut Vec<u8>, s: &str, len: usize) {
    let bytes = s.as_bytes();
    let copy_len = bytes.len().min(len);
    buf.extend_from_slice(&bytes[..copy_len]);
    buf.extend(std::iter::repeat(0_u8).take(len - copy_len));
}

fn info_ifil() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&2_i16.to_le_bytes());
    data.extend_from_slice(&1_i16.to_le_bytes());
    chunk(b"ifil", data)
}

fn info_text(id: &[u8; 4], text: &str) -> Vec<u8> {
    // Null-terminate like real SF2 files, and pad to an even length ourselves so this
    // never depends on `chunk`'s (unreachable-in-practice) pad-byte fallback.
    let mut data = text.as_bytes().to_vec();
    data.push(0);
    if data.len() % 2 != 0 {
        data.push(0);
    }
    chunk(id, data)
}

fn wave_bytes(samples: &[i16]) -> Vec<u8> {
    let mut data = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        data.extend_from_slice(&sample.to_le_bytes());
    }
    data
}

fn bag_bytes(bag: &[(u16, u16)]) -> Vec<u8> {
    let mut data = Vec::with_capacity(bag.len() * 4);
    for (generator_index, modulator_index) in bag {
        data.extend_from_slice(&generator_index.to_le_bytes());
        data.extend_from_slice(&modulator_index.to_le_bytes());
    }
    data
}

fn gen_bytes(generators: &[(u16, i16)]) -> Vec<u8> {
    let mut data = Vec::with_capacity(generators.len() * 4);
    for (generator_type, value) in generators {
        data.extend_from_slice(&generator_type.to_le_bytes());
        data.extend_from_slice(&(*value as u16).to_le_bytes());
    }
    data
}

/// A single all-zero terminal modulator record. The parser discards pmod/imod
/// contents outright, but a real terminal record is included for spec fidelity.
fn terminal_modulator() -> Vec<u8> {
    vec![0_u8; 10]
}

fn phdr_bytes(presets: &[PresetInfoRecord]) -> Vec<u8> {
    let mut data = Vec::with_capacity(presets.len() * 38);
    for preset in presets {
        push_fixed_str(&mut data, &preset.name, 20);
        data.extend_from_slice(&(preset.patch as u16).to_le_bytes());
        data.extend_from_slice(&(preset.bank as u16).to_le_bytes());
        data.extend_from_slice(&preset.zone_start_index.to_le_bytes());
        data.extend_from_slice(&0_i32.to_le_bytes()); // library
        data.extend_from_slice(&0_i32.to_le_bytes()); // genre
        data.extend_from_slice(&0_i32.to_le_bytes()); // morphology
    }
    data
}

fn inst_bytes(instruments: &[InstrumentInfoRecord]) -> Vec<u8> {
    let mut data = Vec::with_capacity(instruments.len() * 22);
    for instrument in instruments {
        push_fixed_str(&mut data, &instrument.name, 20);
        data.extend_from_slice(&instrument.zone_start_index.to_le_bytes());
    }
    data
}

fn shdr_bytes(samples: &[SampleHeaderRecord]) -> Vec<u8> {
    let mut data = Vec::with_capacity(samples.len() * 46);
    for sample in samples {
        push_fixed_str(&mut data, &sample.name, 20);
        data.extend_from_slice(&sample.start.to_le_bytes());
        data.extend_from_slice(&sample.end.to_le_bytes());
        data.extend_from_slice(&sample.start_loop.to_le_bytes());
        data.extend_from_slice(&sample.end_loop.to_le_bytes());
        data.extend_from_slice(&sample.sample_rate.to_le_bytes());
        data.push(sample.original_pitch);
        data.push(0_u8); // pitch correction
        data.extend_from_slice(&0_u16.to_le_bytes()); // link
        data.extend_from_slice(&1_u16.to_le_bytes()); // sample_type: mono
    }
    data
}

// --- Convenience fixtures ---------------------------------------------------

fn sine_wave(len: usize, amplitude: i16) -> Vec<i16> {
    (0..len)
        .map(|i| {
            let phase = 2.0 * std::f64::consts::PI * i as f64 / len as f64;
            (phase.sin() * amplitude as f64).round() as i16
        })
        .collect()
}

/// Builds a SoundFont with one looped sine-ish sample, one instrument, a melodic
/// preset at bank 0 / patch 0, and a drum preset at bank 128 / patch 0.
///
/// The two presets use samples with different original pitches so tests can tell
/// which one was selected by inspecting the rendered voice or the sample header.
pub(crate) fn sine_soundfont() -> Arc<SoundFont> {
    let mut builder = SoundFontBuilder::new();

    let wave = sine_wave(64, 12000);
    let melodic_sample = builder.sample("Sine", &wave, 44100, 60, 0, 64);
    let drum_sample = builder.sample("Sine Drum", &wave, 44100, 36, 0, 64);

    let melodic_instrument = builder.instrument(
        "Sine Instrument",
        vec![(vec![(GeneratorType::SAMPLE_MODES, 1)], melodic_sample)],
    );
    let drum_instrument = builder.instrument(
        "Sine Drum Instrument",
        vec![(vec![(GeneratorType::SAMPLE_MODES, 1)], drum_sample)],
    );

    builder.preset("Sine Melodic", 0, 0, vec![(Vec::new(), melodic_instrument)]);
    builder.preset("Sine Drum", 128, 0, vec![(Vec::new(), drum_instrument)]);

    load(builder.build())
}

/// Builds a SoundFont whose single preset triggers two instrument zones on the same
/// key (a stereo-style layer: pan -500 and +500), for voice-stealing tests.
pub(crate) fn layered_soundfont() -> Arc<SoundFont> {
    let mut builder = SoundFontBuilder::new();

    let wave = sine_wave(64, 12000);
    let sample = builder.sample("Sine", &wave, 44100, 60, 0, 64);

    let instrument = builder.instrument(
        "Layered Instrument",
        vec![
            (
                vec![(GeneratorType::SAMPLE_MODES, 1), (GeneratorType::PAN, -500)],
                sample,
            ),
            (
                vec![(GeneratorType::SAMPLE_MODES, 1), (GeneratorType::PAN, 500)],
                sample,
            ),
        ],
    );

    builder.preset("Layered Preset", 0, 0, vec![(Vec::new(), instrument)]);

    load(builder.build())
}

fn load(bytes: Vec<u8>) -> Arc<SoundFont> {
    let mut cursor = Cursor::new(bytes);
    Arc::new(SoundFont::new(&mut cursor).expect("builder must produce a loadable SoundFont"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthesizer::Synthesizer;
    use crate::synthesizer_settings::SynthesizerSettings;

    #[test]
    fn minimal_soundfont_loads_without_warnings() {
        let mut builder = SoundFontBuilder::new();
        let wave = sine_wave(32, 10000);
        let sample_index = builder.sample("Sine", &wave, 44100, 60, 0, 32);
        let instrument_index = builder.instrument(
            "Instrument",
            vec![(vec![(GeneratorType::SAMPLE_MODES, 1)], sample_index)],
        );
        builder.preset("Preset", 0, 0, vec![(Vec::new(), instrument_index)]);

        let sound_font = load(builder.build());

        assert!(sound_font.get_warnings().is_empty());
        assert_eq!(sound_font.get_presets().len(), 1);
        assert_eq!(sound_font.get_instruments().len(), 1);

        let preset = &sound_font.get_presets()[0];
        assert_eq!(preset.get_name(), "Preset");
        assert_eq!(preset.get_bank_number(), 0);
        assert_eq!(preset.get_patch_number(), 0);
        assert_eq!(preset.get_regions().len(), 1);

        let instrument = &sound_font.get_instruments()[0];
        assert_eq!(instrument.get_name(), "Instrument");
        assert_eq!(instrument.get_regions().len(), 1);

        let sample = &sound_font.get_sample_headers()[0];
        assert_eq!(sample.get_name(), "Sine");
        assert_eq!(sample.get_start(), 0);
        assert_eq!(sample.get_end(), 32);
        assert_eq!(sample.get_start_loop(), 0);
        assert_eq!(sample.get_end_loop(), 32);
        assert_eq!(sample.get_sample_rate(), 44100);
        assert_eq!(sample.get_original_pitch(), 60);
    }

    #[test]
    fn sine_soundfont_has_distinguishable_melodic_and_drum_presets() {
        let sound_font = sine_soundfont();

        assert!(sound_font.get_warnings().is_empty());
        assert_eq!(sound_font.get_presets().len(), 2);

        let melodic = &sound_font.get_presets()[0];
        assert_eq!(melodic.get_bank_number(), 0);
        assert_eq!(melodic.get_patch_number(), 0);

        let drum = &sound_font.get_presets()[1];
        assert_eq!(drum.get_bank_number(), 128);
        assert_eq!(drum.get_patch_number(), 0);

        let melodic_sample_id = sound_font.get_instruments()
            [melodic.get_regions()[0].get_instrument_id()]
        .get_regions()[0]
            .get_sample_id();
        let drum_sample_id = sound_font.get_instruments()[drum.get_regions()[0].get_instrument_id()]
            .get_regions()[0]
            .get_sample_id();

        assert_ne!(
            sound_font.get_sample_headers()[melodic_sample_id].get_original_pitch(),
            sound_font.get_sample_headers()[drum_sample_id].get_original_pitch()
        );
    }

    #[test]
    fn layered_soundfont_triggers_two_panned_zones_for_one_key() {
        let sound_font = layered_soundfont();

        assert!(sound_font.get_warnings().is_empty());
        let instrument = &sound_font.get_instruments()[0];
        assert_eq!(instrument.get_regions().len(), 2);

        let pans: Vec<f32> = instrument
            .get_regions()
            .iter()
            .map(|region| region.get_pan())
            .collect();
        assert!(pans.contains(&-50.0));
        assert!(pans.contains(&50.0));
    }

    #[test]
    fn synthesizer_renders_non_silent_audio_after_note_on() {
        let sound_font = sine_soundfont();
        let settings = SynthesizerSettings::new(44100);
        let mut synthesizer = Synthesizer::new(&sound_font, &settings).unwrap();

        synthesizer.note_on(0, 60, 100);

        let mut left = vec![0_f32; 4096];
        let mut right = vec![0_f32; 4096];
        synthesizer.render(&mut left, &mut right);

        assert!(left.iter().any(|&s| s.abs() > 1e-6));
        assert!(right.iter().any(|&s| s.abs() > 1e-6));
    }
}
