use crate::error::SoundFontError;
use crate::generator::Generator;
use crate::modulator::Modulator;
use crate::zone_info::ZoneInfo;

#[non_exhaustive]
pub(crate) struct Zone {
    pub(crate) generators: Vec<Generator>,
    pub(crate) modulators: Vec<Modulator>,
}

impl Zone {
    pub(crate) fn empty() -> Self {
        Self {
            generators: Vec::new(),
            modulators: Vec::new(),
        }
    }

    /// Builds a zone's generator/modulator slices from the flat pgen/igen and
    /// pmod/imod lists. An out-of-range modulator index/count (corrupt pbag/ibag
    /// data) drops the zone's modulators rather than panicking or failing the
    /// load; the caller counts how many zones this happened to.
    fn new(
        info: &ZoneInfo,
        generators: &[Generator],
        modulators: &[Modulator],
        out_of_range_modulator_zones: &mut usize,
    ) -> Self {
        let mut generator_segment: Vec<Generator> = Vec::new();
        for i in 0..info.generator_count {
            generator_segment.push(generators[(info.generator_index + i) as usize]);
        }

        let modulator_segment = if info.modulator_index < 0
            || info.modulator_count < 0
            || (info.modulator_index + info.modulator_count) as usize > modulators.len()
        {
            *out_of_range_modulator_zones += 1;
            Vec::new()
        } else {
            let start = info.modulator_index as usize;
            let end = start + info.modulator_count as usize;
            modulators[start..end].to_vec()
        };

        Self {
            generators: generator_segment,
            modulators: modulator_segment,
        }
    }

    pub(crate) fn create(
        infos: &[ZoneInfo],
        generators: &[Generator],
        modulators: &[Modulator],
        warnings: &mut Vec<String>,
    ) -> Result<Vec<Zone>, SoundFontError> {
        if infos.len() <= 1 {
            return Err(SoundFontError::ZoneNotFound);
        }

        // The last one is the terminator.
        let count = infos.len() - 1;

        let mut out_of_range_modulator_zones = 0_usize;
        let mut zones: Vec<Zone> = Vec::new();
        for info in infos.iter().take(count) {
            zones.push(Zone::new(
                info,
                generators,
                modulators,
                &mut out_of_range_modulator_zones,
            ));
        }

        if out_of_range_modulator_zones > 0 {
            warnings.push(format!(
                "{} zone(s) had an out-of-range modulator index/count; their modulators were dropped",
                out_of_range_modulator_zones
            ));
        }

        Ok(zones)
    }
}
