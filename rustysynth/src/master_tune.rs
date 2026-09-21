/// The master tuning offsets in semitones, split by which channels they reach.
///
/// Master Coarse Tune (and the GS Master Key Shift that writes the same parameter) transposes
/// the melodic parts only, since transposing a drum kit would change which instrument each key
/// plays. Fine tune and the tuning set through the API are pitch offsets rather than
/// transposition, so they reach every channel.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MasterTune {
    all_channels: f32,
    melodic_only: f32,
}

impl MasterTune {
    pub(crate) fn new(all_channels: f32, melodic_only: f32) -> Self {
        Self {
            all_channels,
            melodic_only,
        }
    }

    pub(crate) fn for_channel(&self, is_percussion: bool) -> f32 {
        if is_percussion {
            self.all_channels
        } else {
            self.all_channels + self.melodic_only
        }
    }
}
