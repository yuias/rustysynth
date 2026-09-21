/// MIDI system selected by the last GM System On, GS Reset or XG System On
/// message. Decides how Bank Select is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemMode {
    Gm,
    Gs,
    Xg,
}
