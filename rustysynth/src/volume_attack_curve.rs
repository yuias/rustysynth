/// Specifies the shape of the volume envelope attack stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeAttackCurve {
    /// Amplitude rises linearly over the attack time, as in the SF2 specification and
    /// most other SoundFont synthesizers.
    Linear,
    /// Amplitude follows `1 - (1 - t)^3`, reaching half amplitude at about 21% of the
    /// attack time. Attacks sound shorter and punchier than with `Linear`.
    Cubic,
}
