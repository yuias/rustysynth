# Changelog (fork)

Changes in this fork relative to upstream rustysynth v1.3.6.
The upstream history is kept unchanged in `CHANGELOG.md`.

# Latest

## Added

- MIDI control changes:
  - Bank Select LSB (CC#32), stored and exposed; used for preset lookup in XG mode.
  - Portamento Time (CC#5), Portamento On/Off (CC#65) and Portamento Control (CC#84).
  - Sostenuto (CC#66): sustains only the notes whose keys are held when the pedal goes down.
  - Soft Pedal (CC#67) and Variation Send (CC#94), stored and exposed without an audible effect yet.
  - Filter Resonance (CC#71) and Brightness (CC#74), applied to sounding voices.
  - Release Time (CC#72), Attack Time (CC#73) and Decay Time (CC#75), applied at note-on.
  - Channel Pressure, applied as vibrato depth (SF2 default modulator).
  - Polyphonic Key Pressure, available as a SoundFont modulator source.
- NRPN handling for GS/XG tone parameters (MSB 1): vibrato rate, depth and delay, TVF cutoff and resonance, and TVA attack, decay and release.
- SysEx processing via `Synthesizer::process_sysex`:
  - GM System On, GS Reset and XG System On.
  - Universal Master Volume, Master Fine Tune and Master Coarse Tune.
  - GS Use for Rhythm Part (switches a part between melodic and drum banks).
  - GS Scale Tuning (all 12 notes of a part at once).
  - GS drum instrument NRPNs, which address one note of a drum kit: pitch coarse, level, panpot, reverb send and chorus send. A panpot of 0 asks for a random position and is taken as centre. Like the other GS NRPNs, these survive Reset All Controllers.
  - GS Master Volume, Master Key Shift, Reverb Macro, Reverb Level and Chorus Macro (Chorus Level is not supported).
  - XG Part Mode (switches a MIDI channel between melodic and drum banks).
  - GS and XG messages are accepted for device IDs 10h-1Fh.
- SysEx events in MIDI files are sent to the synthesizer during `MidiFileSequencer` playback. SysEx split across `F0`/`F7` packets is reassembled.
- Master tuning: `Synthesizer::set_master_tune` / `get_master_tune`.
- Per-channel scale tuning: `Synthesizer::set_scale_tuning` / `get_scale_tuning`.
- `Synthesizer::set_percussion_channel` to switch a channel between melodic and drum banks at runtime.
- Channel state access: `Channel` is public and `Synthesizer::get_channel` returns per-channel controller values (normalized and raw 0-127 getters).
- Channel mute: `set_channel_mute`, `is_channel_muted`, `set_channel_mute_mask`, `get_channel_mute_mask`.
- Effect parameters: `set_reverb_room_size`, `set_reverb_damp`, `set_reverb_wet`, `set_reverb_width`, `set_chorus_params`, `set_chorus_type` (six GM chorus presets) and `set_chorus_feedback`.
- SF2 default modulator from note-on velocity to filter cutoff.
- SoundFont modulators (PMOD/IMOD). Instrument modulators override the SF2 default modulators and preset modulators add to them. Sources: note-on velocity and key, poly and channel pressure, pitch wheel and its sensitivity, and MIDI controllers. Modulators on pitch, filter, attenuation, pan and effect sends follow controller changes on sounding notes, unless all of their sources are fixed at note-on, in which case they take effect from the first sample instead of being smoothed in; modulators on envelope and LFO timing are applied at note-on. Modulator chains are not supported and are skipped; the loader reports a chain apart from the link failures the specification itself rejects, which are a link used as an amount source, a link naming a modulator the zone does not have, and a link source that nothing feeds.
- `SynthesizerSettings::volume_attack_curve` (`VolumeAttackCurve::Linear` or `Cubic`), `SynthesizerSettings::enable_velocity_to_filter_cutoff`, `SynthesizerSettings::enable_soundfont_modulators`, `SynthesizerSettings::enable_master_coarse_tune_on_percussion` and `SynthesizerSettings::enable_generator_range_clamp`.
- `SoundFont::get_warnings` lists instrument regions and modulators that were skipped while loading.
- System mode (GM/GS/XG), selected by the last GM System On, GS Reset or XG System On message; `Synthesizer::reset` returns it to GM. In XG mode, Bank Select LSB selects the bank of melodic channels, and Bank Select MSB 127 or 126 switches the channel to the drum bank while any other MSB switches it back to melodic, overriding `set_percussion_channel` and the default drum channel. A channel switched to drums this way persists across `Synthesizer::reset`, exactly like one set through `set_percussion_channel` or GS Use for Rhythm Part; only a GM/GS/XG reset message or a later Bank Select MSB changes it back.

## Changed

- Sample interpolation uses 4-point Hermite instead of linear interpolation.
- Vibrato and modulation LFO pitch changes are interpolated per sample instead of per block.
- The low-pass filter is a Cytomic TPT state variable filter instead of a Direct Form I biquad.
- Reverb is an 8-line Hadamard feedback delay network instead of Freeverb. Decay time and wet level are calibrated to the previous implementation, but the tone, early reflection timing and stereo image differ.
- Chorus uses three modulated voices per channel with optional feedback.
- The volume envelope attack defaults to a cubic curve (`VolumeAttackCurve::Cubic`), which sounds shorter than the previous linear ramp. Set `VolumeAttackCurve::Linear` for the previous behavior.
- Soft notes are filtered by the velocity to filter cutoff default modulator, up to two octaves at the lowest velocity. Set `enable_velocity_to_filter_cutoff` to `false` for the previous behavior.
- When polyphony is exhausted, voice stealing prefers voices on the same channel, and above all the same key on the same channel.
- Invalid instrument regions are skipped with a warning instead of rejecting the whole SoundFont. `SoundFontError::SanityCheckFailed` and `SoundFontError::InvalidSampleId` are no longer returned; a region naming a sample that does not exist is skipped like any other invalid region. Regions that never loop keep playing even when their loop points are unusable, since a non-looping region never reads them.
- SoundFonts that define modulators sound as their authors specified. For example, GeneralUser GS disables the velocity to filter cutoff modulator on most instruments, and TimGM6mb disables modulation wheel vibrato on some. Set `enable_soundfont_modulators` to `false` for the previous behavior. Modulator amounts for reverb and chorus sends are relative to the SF2 default amount of 200, which corresponds to the existing send level.
- Generator values that leave the useful range the SoundFont specification gives them are clamped to the nearest value in range, as the specification asks, whether the value is written that way or results from summing the preset, instrument and note-on modulator values. This covers the filter cutoff and resonance, the pitch and filter modulation depths, the LFO and envelope times, the sustain levels, the effect sends, pan, initial attenuation and scale tuning. Coarse and fine tune are left alone, since the pitch of a sounding note is offset outside the region. SoundFonts that sum out of range sound different as a result; GeneralUser GS does so on several hundred regions, most audibly where it asks for a negative attenuation or sustain level, which would be louder than the sample itself. Set `enable_generator_range_clamp` to `false` for the previous behavior; that is not conforming and is only meant for a SoundFont that relied on out-of-range values.
- GM/GS/XG reset messages restore channel 10 as the only drum channel. `Synthesizer::reset` keeps drum channels set with `set_percussion_channel`, and keeps the master volume and tuning set through the API.
- Effect parameters set by GS reverb/chorus macros persist across `Synthesizer::reset`, like parameters set through the API.
- Master Coarse Tune, and the GS Master Key Shift that writes the same parameter, no longer transpose percussion channels, matching hardware: transposing a drum kit changes which instrument each key plays. Set `enable_master_coarse_tune_on_percussion` to `true` for the previous behavior. Master Fine Tune and `Synthesizer::set_master_tune` are unaffected and still reach every channel.

## Fixed

- A panic when the pitch ratio exceeded the length of a very short sample loop.
- MIDI files with SMPTE time division were read as ticks per beat and played at the wrong speed.
- Running status was not cancelled by meta events, so bytes following one were read as a message with a meta status byte.
- MIDI files declaring a time division of zero made every event time infinite or NaN. The header is now rejected as invalid.
