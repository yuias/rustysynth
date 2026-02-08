#![allow(dead_code)]

use std::io::Read;

use crate::binary_reader::BinaryReader;
use crate::error::SoundFontError;
use crate::four_cc::FourCC;
use crate::instrument::Instrument;
use crate::preset::Preset;
use crate::sample_header::SampleHeader;
use crate::soundfont_info::SoundFontInfo;
use crate::soundfont_parameters::SoundFontParameters;
use crate::soundfont_sampledata::SoundFontSampleData;
use crate::LoopMode;

/// Reperesents a SoundFont.
#[derive(Debug)]
#[non_exhaustive]
pub struct SoundFont {
    pub(crate) info: SoundFontInfo,
    pub(crate) bits_per_sample: i32,
    pub(crate) wave_data: Vec<i16>,
    pub(crate) sample_headers: Vec<SampleHeader>,
    pub(crate) presets: Vec<Preset>,
    pub(crate) instruments: Vec<Instrument>,
    warnings: Vec<String>,
}

impl SoundFont {
    /// Loads a SoundFont from the stream.
    ///
    /// # Arguments
    ///
    /// * `reader` - The data stream used to load the SoundFont.
    pub fn new<R: Read>(reader: &mut R) -> Result<Self, SoundFontError> {
        let chunk_id = BinaryReader::read_four_cc(reader)?;
        if chunk_id != b"RIFF" {
            return Err(SoundFontError::RiffChunkNotFound);
        }

        let _size = BinaryReader::read_i32(reader)?;

        let form_type = BinaryReader::read_four_cc(reader)?;
        if form_type != b"sfbk" {
            return Err(SoundFontError::InvalidRiffChunkType {
                expected: FourCC::from_bytes(*b"sfbk"),
                actual: form_type,
            });
        }

        let info = SoundFontInfo::new(reader)?;
        let sample_data = SoundFontSampleData::new(reader)?;
        let parameters = SoundFontParameters::new(reader)?;

        let mut sound_font = Self {
            info,
            bits_per_sample: sample_data.bits_per_sample,
            wave_data: sample_data.wave_data,
            sample_headers: parameters.sample_headers,
            presets: parameters.presets,
            instruments: parameters.instruments,
            warnings: Vec::new(),
        };

        sound_font.sanitize();

        Ok(sound_font)
    }

    /// Remove invalid instrument regions and collect warnings.
    ///
    /// References:
    /// - https://github.com/sinshu/rustysynth/issues/22
    /// - https://github.com/sinshu/rustysynth/issues/33
    /// - https://github.com/sinshu/rustysynth/pull/51
    fn sanitize(&mut self) {
        let wave_len = self.wave_data.len();
        let mut warnings = Vec::new();

        for instrument in &mut self.instruments {
            let before = instrument.regions.len();
            let inst_name = instrument.name.clone();
            instrument.regions.retain(|region| {
                let start = region.get_sample_start();
                let end = region.get_sample_end();
                let start_loop = region.get_sample_start_loop();
                let end_loop = region.get_sample_end_loop();
                let loop_mode = region.get_sample_modes();

                if start < 0 {
                    warnings.push(format!(
                        "instrument '{}': region removed (sample_start {} < 0)",
                        inst_name, start
                    ));
                    return false;
                }
                if start_loop < 0 {
                    warnings.push(format!(
                        "instrument '{}': region removed (sample_start_loop {} < 0)",
                        inst_name, start_loop
                    ));
                    return false;
                }
                if end as usize >= wave_len {
                    warnings.push(format!(
                        "instrument '{}': region removed (sample_end {} >= wave_data len {})",
                        inst_name, end, wave_len
                    ));
                    return false;
                }
                if end_loop as usize >= wave_len {
                    warnings.push(format!(
                        "instrument '{}': region removed (sample_end_loop {} >= wave_data len {})",
                        inst_name, end_loop, wave_len
                    ));
                    return false;
                }
                if end <= start {
                    warnings.push(format!(
                        "instrument '{}': region removed (sample_end {} <= sample_start {})",
                        inst_name, end, start
                    ));
                    return false;
                }
                if end_loop < start_loop {
                    warnings.push(format!(
                        "instrument '{}': region removed (end_loop {} < start_loop {})",
                        inst_name, end_loop, start_loop
                    ));
                    return false;
                }
                if loop_mode != LoopMode::NoLoop && start_loop >= end_loop {
                    warnings.push(format!(
                        "instrument '{}': region removed (loop mode active but start_loop {} >= end_loop {})",
                        inst_name, start_loop, end_loop
                    ));
                    return false;
                }
                true
            });

            let removed = before - instrument.regions.len();
            if removed > 0 && instrument.regions.is_empty() {
                warnings.push(format!(
                    "instrument '{}': all {} regions removed",
                    inst_name, before
                ));
            }
        }

        self.warnings = warnings;
    }

    /// Gets the information of the SoundFont.
    pub fn get_info(&self) -> &SoundFontInfo {
        &self.info
    }

    /// Gets the bits per sample of the sample data.
    pub fn get_bits_per_sample(&self) -> i32 {
        self.bits_per_sample
    }

    /// Gets the sample data.
    pub fn get_wave_data(&self) -> &[i16] {
        &self.wave_data[..]
    }

    /// Gets the samples of the SoundFont.
    pub fn get_sample_headers(&self) -> &[SampleHeader] {
        &self.sample_headers[..]
    }

    /// Gets the presets of the SoundFont.
    pub fn get_presets(&self) -> &[Preset] {
        &self.presets[..]
    }

    /// Gets the instruments of the SoundFont.
    pub fn get_instruments(&self) -> &[Instrument] {
        &self.instruments[..]
    }

    /// Gets the warnings generated during loading (e.g. invalid regions that were skipped).
    pub fn get_warnings(&self) -> &[String] {
        &self.warnings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs::File, path::PathBuf};

    fn samples_dir_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("samples")
    }

    #[test]
    fn test_load_reject_sf3() {
        let path = samples_dir_path().join("dummy.sf3");
        let mut file = File::open(&path).unwrap();
        assert!(matches!(
            SoundFont::new(&mut file),
            Err(SoundFontError::UnsupportedSampleFormat)
        ));
    }

    // smpl sub-chunk exists, but is zero-length.
    #[test]
    fn test_load_empty_samples() {
        let path = samples_dir_path().join("test_empty_samples.sf2");
        let mut file = File::open(&path).unwrap();
        assert!(matches!(
            SoundFont::new(&mut file),
            Err(SoundFontError::SampleDataNotFound)
        ));
    }
}
