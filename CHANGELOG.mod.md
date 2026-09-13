# Changelog (fork)

Changes in this fork relative to upstream rustysynth v1.3.6.
The upstream history is kept unchanged in `CHANGELOG.md`.

# Latest

## Added

- MIDI control changes:
  - Bank Select LSB (CC#32), stored and exposed; preset lookup still uses the MSB only.
  - Portamento Time (CC#5), Portamento On/Off (CC#65) and Portamento Control (CC#84).
  - Sostenuto (CC#66), Soft Pedal (CC#67) and Variation Send (CC#94), stored and exposed without an audible effect yet.
  - Filter Resonance (CC#71) and Brightness (CC#74), applied to sounding voices.
  - Release Time (CC#72), Attack Time (CC#73) and Decay Time (CC#75), applied at note-on.
  - Channel Pressure, applied as vibrato depth (SF2 default modulator).
- NRPN handling for GS/XG vibrato rate, depth and delay (MSB 1), applied at note-on.
- SysEx processing via `Synthesizer::process_sysex`:
  - GM System On, GS Reset and XG System On.
  - Universal Master Volume, Master Fine Tune and Master Coarse Tune.
  - GS Scale Tuning (all 12 notes of a part at once).
- Master tuning: `Synthesizer::set_master_tune` / `get_master_tune`.
- Per-channel scale tuning: `Synthesizer::set_scale_tuning` / `get_scale_tuning`.
- `Synthesizer::set_percussion_channel` to switch a channel between melodic and drum banks at runtime.
- Channel state access: `Channel` is public and `Synthesizer::get_channel` returns per-channel controller values (normalized and raw 0-127 getters).
- Channel mute: `set_channel_mute`, `is_channel_muted`, `set_channel_mute_mask`, `get_channel_mute_mask`.
- Effect parameters: `set_reverb_room_size`, `set_reverb_damp`, `set_reverb_wet`, `set_reverb_width`, `set_chorus_params`, `set_chorus_type` (six GM chorus presets) and `set_chorus_feedback`.
- SF2 default modulator from note-on velocity to filter cutoff.
- `SynthesizerSettings::volume_attack_curve` (`VolumeAttackCurve::Linear` or `Cubic`) and `SynthesizerSettings::enable_velocity_to_filter_cutoff`.
- `SoundFont::get_warnings` lists instrument regions that were skipped while loading.

## Changed

- Sample interpolation uses 4-point Hermite instead of linear interpolation.
- Vibrato and modulation LFO pitch changes are interpolated per sample instead of per block.
- The low-pass filter is a Cytomic TPT state variable filter instead of a Direct Form I biquad.
- Reverb is an 8-line Hadamard feedback delay network instead of Freeverb. Decay time and wet level are calibrated to the previous implementation, but the tone, early reflection timing and stereo image differ.
- Chorus uses three modulated voices per channel with optional feedback.
- The volume envelope attack defaults to a cubic curve (`VolumeAttackCurve::Cubic`), which sounds shorter than the previous linear ramp. Set `VolumeAttackCurve::Linear` for the previous behavior.
- Soft notes are filtered by the velocity to filter cutoff default modulator, up to two octaves at the lowest velocity. Set `enable_velocity_to_filter_cutoff` to `false` for the previous behavior.
- When polyphony is exhausted, voice stealing prefers voices on the same channel, and above all the same key on the same channel.
- Invalid instrument regions are skipped with a warning instead of rejecting the whole SoundFont. `SoundFontError::SanityCheckFailed` is no longer returned.
- GM/GS/XG reset messages restore channel 10 as the only drum channel. `Synthesizer::reset` keeps drum channels set with `set_percussion_channel`, and keeps the master volume and tuning set through the API.

## Fixed

- A panic when the pitch ratio exceeded the length of a very short sample loop.
