#![allow(dead_code)]

use std::io::Read;

use crate::binary_reader::BinaryReader;
use crate::four_cc::FourCC;
use crate::read_counter::ReadCounter;
use crate::MidiFileError;
use crate::MidiFileLoopType;

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub(crate) enum Message {
    Normal { status: u8, data1: u8, data2: u8 },
    TempoChange { bytes: [u8; 3] },
    // The payload lives in `MidiFile::sysex_data`; `bytes` is a big-endian u24
    // index into it. An index keeps this variant within the 4-byte budget
    // (see `test_message_size`) while indices remain stable across
    // `merge_tracks` reordering and the `LoopPoint` insertion into track 0.
    SysEx { bytes: [u8; 3] },
    LoopStart,
    LoopEnd,
    EndOfTrack,
}

impl Message {
    pub(crate) fn common1(status: u8, data1: u8) -> Self {
        Self::Normal {
            status,
            data1,
            data2: 0,
        }
    }

    pub(crate) fn common2(status: u8, data1: u8, data2: u8, loop_type: MidiFileLoopType) -> Self {
        let command = status & 0xF0;

        if command == 0xB0 {
            match loop_type {
                MidiFileLoopType::RpgMaker => {
                    if data1 == 111 {
                        return Message::LoopStart;
                    }
                }

                MidiFileLoopType::IncredibleMachine => {
                    if data1 == 110 {
                        return Message::LoopStart;
                    }
                    if data1 == 111 {
                        return Message::LoopEnd;
                    }
                }

                MidiFileLoopType::FinalFantasy => {
                    if data1 == 116 {
                        return Message::LoopStart;
                    }
                    if data1 == 117 {
                        return Message::LoopEnd;
                    }
                }

                _ => (),
            }
        }

        Self::Normal {
            status,
            data1,
            data2,
        }
    }

    pub(crate) fn tempo_change(tempo: i32) -> Self {
        // Truncate to u24
        let bytes = tempo.to_be_bytes()[1..].try_into().unwrap();
        Self::TempoChange { bytes }
    }

    /// Builds a message referencing a SysEx payload at `index` in `MidiFile::sysex_data`.
    /// Fails if `index` does not fit in the u24 the message can carry, which would
    /// require a MIDI file with over 16 million SysEx events.
    fn sysex(index: usize) -> Result<Self, MidiFileError> {
        if index > 0xFF_FFFF {
            return Err(MidiFileError::InvalidChunkData(FourCC::from_bytes(
                *b"MTrk",
            )));
        }
        let bytes = (index as u32).to_be_bytes()[1..].try_into().unwrap();
        Ok(Self::SysEx { bytes })
    }

    pub(crate) fn sysex_index(bytes: [u8; 3]) -> usize {
        u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]) as usize
    }
}

/// How tick positions in a track map to elapsed time.
#[derive(Clone, Copy, Debug)]
enum TimeDivision {
    /// Ticks per quarter note; `Message::TempoChange` events scale elapsed time.
    TicksPerQuarterNote(i32),
    /// SMPTE time code: a tick is a fixed `1 / (fps * ticks_per_frame)` seconds,
    /// regardless of any tempo meta events.
    Smpte { fps: f64, ticks_per_frame: f64 },
}

/// Represents a standard MIDI file.
#[derive(Debug)]
#[non_exhaustive]
pub struct MidiFile {
    pub(crate) messages: Vec<Message>,
    pub(crate) times: Vec<f64>,
    // Side table for SysEx payloads (without the leading F0 or trailing F7),
    // referenced by `Message::SysEx` via index. Kept out of `Message` to
    // preserve its 4-byte size.
    pub(crate) sysex_data: Vec<Box<[u8]>>,
}

impl MidiFile {
    /// Loads a MIDI file from the stream.
    ///
    /// # Arguments
    ///
    /// * `reader` - The data stream used to load the MIDI file.
    pub fn new<R: Read>(reader: &mut R) -> Result<Self, MidiFileError> {
        MidiFile::new_with_loop_type(reader, MidiFileLoopType::LoopPoint(0))
    }

    /// Loads a MIDI file from the stream with a specified loop type.
    ///
    /// # Arguments
    ///
    /// * `reader` - The data stream used to load the MIDI file.
    /// * `loop_type` - The type of the loop extension to be used.
    ///
    /// # Remarks
    ///
    /// `MidiFileLoopType` has the following variants:
    /// * `LoopPoint(usize)` - Specifies the loop start point by a tick value.
    /// * `RpgMaker` - The RPG Maker style loop.
    ///   CC #111 will be the loop start point.
    /// * `IncredibleMachine` - The Incredible Machine style loop.
    ///   CC #110 and #111 will be the start and end points of the loop.
    /// * `FinalFantasy` - The Final Fantasy style loop.
    ///   CC #116 and #117 will be the start and end points of the loop.
    pub fn new_with_loop_type<R: Read>(
        reader: &mut R,
        loop_type: MidiFileLoopType,
    ) -> Result<Self, MidiFileError> {
        let chunk_type = BinaryReader::read_four_cc(reader)?;
        if chunk_type != b"MThd" {
            return Err(MidiFileError::InvalidChunkType {
                expected: FourCC::from_bytes(*b"MThd"),
                actual: chunk_type,
            });
        }

        let size = BinaryReader::read_i32_big_endian(reader)?;
        if size != 6 {
            return Err(MidiFileError::InvalidChunkData(FourCC::from_bytes(
                *b"MThd",
            )));
        }

        let format = BinaryReader::read_i16_big_endian(reader)?;
        if !(format == 0 || format == 1) {
            return Err(MidiFileError::UnsupportedFormat(format));
        }

        let track_count = BinaryReader::read_i16_big_endian(reader)? as i32;
        let division = BinaryReader::read_i16_big_endian(reader)?;

        // Either resolution reaching zero would make every delta time infinite or NaN, and
        // there is no sensible resolution to substitute, so the header is rejected. fps needs
        // no guard: a negative division means a negative fps_code, so fps is at least 1.
        if division == 0 || (division < 0 && division.to_be_bytes()[1] == 0) {
            return Err(MidiFileError::InvalidChunkData(FourCC::from_bytes(
                *b"MThd",
            )));
        }

        // A negative division means SMPTE time code: the high byte (as a signed
        // value) is the negated frames-per-second, and the low byte is the
        // number of ticks per frame. Positive division is the usual
        // ticks-per-quarter-note.
        let time_division = if division < 0 {
            let [fps_code, ticks_per_frame] = division.to_be_bytes();
            let fps_code = fps_code as i8;
            let fps = if fps_code == -29 {
                29.97
            } else {
                -(fps_code as f64)
            };
            TimeDivision::Smpte {
                fps,
                ticks_per_frame: ticks_per_frame as f64,
            }
        } else {
            TimeDivision::TicksPerQuarterNote(division as i32)
        };

        let mut message_lists: Vec<Vec<Message>> = Vec::new();
        let mut tick_lists: Vec<Vec<i32>> = Vec::new();
        let mut sysex_data: Vec<Box<[u8]>> = Vec::new();

        for _i in 0..track_count {
            let (message_list, tick_list) =
                MidiFile::read_track(reader, loop_type, &mut sysex_data)?;
            message_lists.push(message_list);
            tick_lists.push(tick_list);
        }

        match loop_type {
            MidiFileLoopType::LoopPoint(loop_point) if loop_point != 0 => {
                let loop_point = loop_point as i32;
                let tick_list = &mut tick_lists[0];
                let message_list = &mut message_lists[0];

                if loop_point <= *tick_list.last().unwrap() {
                    for i in 0..tick_list.len() {
                        if tick_list[i] >= loop_point {
                            tick_list.insert(i, loop_point);
                            message_list.insert(i, Message::LoopStart);
                            break;
                        }
                    }
                } else {
                    tick_list.push(loop_point);
                    message_list.push(Message::LoopStart);
                }
            }
            _ => (),
        }

        let (messages, times) = MidiFile::merge_tracks(&message_lists, &tick_lists, time_division);

        Ok(Self {
            messages,
            times,
            sysex_data,
        })
    }

    fn discard_data<R: Read>(reader: &mut R) -> Result<(), MidiFileError> {
        let size = BinaryReader::read_i32_variable_length(reader)? as usize;
        BinaryReader::discard_data(reader, size)?;
        Ok(())
    }

    /// Reads a variable-length size prefix followed by that many bytes.
    fn read_sized_data<R: Read>(reader: &mut R) -> Result<Vec<u8>, MidiFileError> {
        let size = BinaryReader::read_i32_variable_length(reader)? as usize;
        let mut data = vec![0_u8; size];
        reader.read_exact(&mut data)?;
        Ok(data)
    }

    /// Stores a SysEx payload and appends a referencing message at `tick`.
    fn emit_sysex(
        sysex_data: &mut Vec<Box<[u8]>>,
        messages: &mut Vec<Message>,
        ticks: &mut Vec<i32>,
        tick: i32,
        payload: Vec<u8>,
    ) -> Result<(), MidiFileError> {
        let index = sysex_data.len();
        sysex_data.push(payload.into_boxed_slice());
        messages.push(Message::sysex(index)?);
        ticks.push(tick);
        Ok(())
    }

    fn read_tempo<R: Read>(reader: &mut R) -> Result<i32, MidiFileError> {
        let size = BinaryReader::read_i32_variable_length(reader)?;
        if size != 3 {
            return Err(MidiFileError::InvalidTempoValue);
        }

        let b1 = BinaryReader::read_u8(reader)? as i32;
        let b2 = BinaryReader::read_u8(reader)? as i32;
        let b3 = BinaryReader::read_u8(reader)? as i32;

        Ok((b1 << 16) | (b2 << 8) | b3)
    }

    fn read_track<R: Read>(
        reader: &mut R,
        loop_type: MidiFileLoopType,
        sysex_data: &mut Vec<Box<[u8]>>,
    ) -> Result<(Vec<Message>, Vec<i32>), MidiFileError> {
        let chunk_type = BinaryReader::read_four_cc(reader)?;
        if chunk_type != b"MTrk" {
            return Err(MidiFileError::InvalidChunkType {
                expected: FourCC::from_bytes(*b"MTrk"),
                actual: chunk_type,
            });
        }

        let size = BinaryReader::read_i32_big_endian(reader)? as usize;
        let reader = &mut ReadCounter::new(reader);

        let mut messages: Vec<Message> = Vec::new();
        let mut ticks: Vec<i32> = Vec::new();

        let mut tick: i32 = 0;
        let mut last_status: u8 = 0;

        // An F0 packet with no trailing F7 is completed by one or more later F7
        // packets. Holds the accumulated payload (without the F0) while waiting.
        let mut pending_sysex: Option<Vec<u8>> = None;

        loop {
            let delta = BinaryReader::read_i32_variable_length(reader)?;
            let first = BinaryReader::read_u8(reader)?;

            tick += delta;

            // Anything other than an F7 continuation means a pending F0 packet
            // was never terminated (e.g. a malformed file, or a new F0 starting
            // before the previous one closed). Flush it as-is rather than
            // silently dropping a GS/XG reset split oddly across packets.
            if first != 0xF7 {
                if let Some(payload) = pending_sysex.take() {
                    MidiFile::emit_sysex(sysex_data, &mut messages, &mut ticks, tick, payload)?;
                }
            }

            if (first & 128) == 0 {
                // With no running status (e.g. right after a SysEx event, which cancels it per
                // the SMF spec), last_status is 0 and the bytes become a message with status 0
                // that the synthesizer ignores. Malformed files keep loading as before.
                let command = last_status & 0xF0;
                if command == 0xC0 || command == 0xD0 {
                    messages.push(Message::common1(last_status, first));
                    ticks.push(tick);
                } else {
                    let data2 = BinaryReader::read_u8(reader)?;
                    messages.push(Message::common2(last_status, first, data2, loop_type));
                    ticks.push(tick);
                }

                continue;
            }

            match first {
                0xF0 => {
                    let mut data = MidiFile::read_sized_data(reader)?;
                    if data.last() == Some(&0xF7) {
                        data.pop();
                        MidiFile::emit_sysex(sysex_data, &mut messages, &mut ticks, tick, data)?;
                    } else {
                        pending_sysex = Some(data);
                    }
                    // SysEx cancels running status per the SMF spec.
                    last_status = 0;
                    continue;
                }
                0xF7 => {
                    let data = MidiFile::read_sized_data(reader)?;
                    if let Some(mut payload) = pending_sysex.take() {
                        payload.extend_from_slice(&data);
                        if payload.last() == Some(&0xF7) {
                            payload.pop();
                            MidiFile::emit_sysex(
                                sysex_data,
                                &mut messages,
                                &mut ticks,
                                tick,
                                payload,
                            )?;
                        } else {
                            pending_sysex = Some(payload);
                        }
                    }
                    // A standalone F7 with no preceding F0 is an escape packet of
                    // raw bytes rather than a SysEx continuation; there is no
                    // consumer for that, so it's discarded.
                    last_status = 0;
                    continue;
                }
                0xFF => {
                    match BinaryReader::read_u8(reader)? {
                        0x2F => {
                            BinaryReader::read_u8(reader)?;
                            messages.push(Message::EndOfTrack);
                            ticks.push(tick);

                            // Some MIDI files may have events inserted after the EOT.
                            // Such events should be ignored.
                            if reader.bytes_read() < size {
                                BinaryReader::discard_data(reader, size - reader.bytes_read())?;
                            }

                            return Ok((messages, ticks));
                        }
                        0x51 => {
                            messages.push(Message::tempo_change(MidiFile::read_tempo(reader)?));
                            ticks.push(tick);
                        }
                        _ => MidiFile::discard_data(reader)?,
                    }

                    // Meta events cancel running status per the SMF spec, the same as SysEx.
                    last_status = 0;
                    continue;
                }
                _ => {
                    let command = first & 0xF0;
                    if command == 0xC0 || command == 0xD0 {
                        let data1 = BinaryReader::read_u8(reader)?;
                        messages.push(Message::common1(first, data1));
                        ticks.push(tick);
                    } else {
                        let data1 = BinaryReader::read_u8(reader)?;
                        let data2 = BinaryReader::read_u8(reader)?;
                        messages.push(Message::common2(first, data1, data2, loop_type));
                        ticks.push(tick);
                    }
                }
            }

            last_status = first
        }
    }

    fn merge_tracks(
        message_lists: &[Vec<Message>],
        tick_lists: &[Vec<i32>],
        time_division: TimeDivision,
    ) -> (Vec<Message>, Vec<f64>) {
        let mut merged_messages: Vec<Message> = Vec::new();
        let mut merged_times: Vec<f64> = Vec::new();

        let mut indices: Vec<usize> = vec![0; message_lists.len()];

        let mut current_tick: i32 = 0;
        let mut current_time: f64 = 0.0;

        let mut tempo: f64 = 120.0;

        loop {
            let mut min_tick = i32::MAX;
            let mut min_index: i32 = -1;

            for ch in 0..tick_lists.len() {
                if indices[ch] < tick_lists[ch].len() {
                    let tick = tick_lists[ch][indices[ch]];
                    if tick < min_tick {
                        min_tick = tick;
                        min_index = ch as i32;
                    }
                }
            }

            if min_index == -1 {
                break;
            }

            let next_tick = tick_lists[min_index as usize][indices[min_index as usize]];
            let delta_tick = next_tick - current_tick;
            let delta_time = match time_division {
                TimeDivision::TicksPerQuarterNote(resolution) => {
                    60.0 / (resolution as f64 * tempo) * delta_tick as f64
                }
                TimeDivision::Smpte {
                    fps,
                    ticks_per_frame,
                } => delta_tick as f64 / (fps * ticks_per_frame),
            };

            current_tick += delta_tick;
            current_time += delta_time;

            let message = message_lists[min_index as usize][indices[min_index as usize]];
            if let Message::TempoChange { bytes } = message {
                // Tempo meta events don't affect SMPTE-timed files.
                if let TimeDivision::TicksPerQuarterNote(_) = time_division {
                    let tempo_i32 = i32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
                    tempo = 60000000.0 / tempo_i32 as f64;
                }
            } else {
                merged_messages.push(message);
                merged_times.push(current_time);
            }

            indices[min_index as usize] += 1;
        }

        (merged_messages, merged_times)
    }

    /// Get the length of the MIDI file in seconds.
    pub fn get_length(&self) -> f64 {
        *self.times.last().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_message_size() {
        // Avoid increasing the size of the Message type
        assert_eq!(size_of::<Message>(), 4);
    }

    /// Builds a single-track, format-0 SMF with the given division and track body.
    /// An End of Track meta event is appended automatically.
    fn build_midi_file(division: i16, mut track_data: Vec<u8>) -> Vec<u8> {
        track_data.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MThd");
        bytes.extend_from_slice(&6_i32.to_be_bytes());
        bytes.extend_from_slice(&0_i16.to_be_bytes());
        bytes.extend_from_slice(&1_i16.to_be_bytes());
        bytes.extend_from_slice(&division.to_be_bytes());
        bytes.extend_from_slice(b"MTrk");
        bytes.extend_from_slice(&(track_data.len() as i32).to_be_bytes());
        bytes.extend_from_slice(&track_data);
        bytes
    }

    fn parse(division: i16, track_data: Vec<u8>) -> Result<MidiFile, MidiFileError> {
        let bytes = build_midi_file(division, track_data);
        MidiFile::new(&mut Cursor::new(bytes))
    }

    #[test]
    fn f0_sysex_with_trailing_f7_yields_payload_without_f7() {
        let payload = [0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41];

        let mut track = vec![0x00, 0xF0, (payload.len() + 1) as u8];
        track.extend_from_slice(&payload);
        track.push(0xF7);

        let midi_file = parse(480, track).unwrap();

        assert_eq!(midi_file.times[0], 0.0);
        match midi_file.messages[0] {
            Message::SysEx { bytes } => {
                let index = Message::sysex_index(bytes);
                assert_eq!(&*midi_file.sysex_data[index], &payload[..]);
            }
            _ => panic!("expected a SysEx message"),
        }
    }

    #[test]
    fn split_f0_f7_sysex_is_reassembled() {
        let payload = [0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41];
        let (first_part, second_part) = payload.split_at(5);

        let mut track = vec![0x00, 0xF0, first_part.len() as u8];
        track.extend_from_slice(first_part);
        track.push(0x00);
        track.push(0xF7);
        track.push((second_part.len() + 1) as u8);
        track.extend_from_slice(second_part);
        track.push(0xF7);

        let midi_file = parse(480, track).unwrap();

        // Only one reassembled SysEx message, not two fragments.
        let sysex_count = midi_file
            .messages
            .iter()
            .filter(|m| matches!(m, Message::SysEx { .. }))
            .count();
        assert_eq!(sysex_count, 1);

        match midi_file.messages[0] {
            Message::SysEx { bytes } => {
                let index = Message::sysex_index(bytes);
                assert_eq!(&*midi_file.sysex_data[index], &payload[..]);
            }
            _ => panic!("expected a SysEx message"),
        }
    }

    #[test]
    fn running_status_is_not_reused_after_sysex() {
        let track = vec![
            0x00, 0x90, 0x3C,
            0x64, // Note On, ch0, key 60, velocity 100 (sets running status)
            0x00, 0xF0, 0x05, 0x7E, 0x7F, 0x09, 0x01, 0xF7, // GM System On
            0x00, 0x3E,
            0x64, // Bare data bytes: would be a Note On only if running status survived
        ];

        let midi_file = parse(480, track).unwrap();
        let note_ons = midi_file
            .messages
            .iter()
            .filter(|message| matches!(message, Message::Normal { status: 0x90, .. }))
            .count();
        assert_eq!(note_ons, 1);
    }

    #[test]
    fn running_status_is_not_reused_after_a_meta_event() {
        let track = vec![
            0x00, 0x90, 0x3C,
            0x64, // Note On, ch0, key 60, velocity 100 (sets running status)
            0x00, 0xFF, 0x01, 0x02, 0x68, 0x69, // Text meta event
            0x00, 0x3E,
            0x64, // Bare data bytes: would be a Note On only if running status survived
        ];

        let midi_file = parse(480, track).unwrap();
        let note_ons = midi_file
            .messages
            .iter()
            .filter(|message| matches!(message, Message::Normal { status: 0x90, .. }))
            .count();
        assert_eq!(note_ons, 1);

        // The bare bytes must not inherit the meta event's FF status either.
        assert!(!midi_file
            .messages
            .iter()
            .any(|message| matches!(message, Message::Normal { status: 0xFF, .. })));
    }

    #[test]
    fn zero_ticks_per_quarter_note_is_rejected() {
        let track = vec![0x00, 0x90, 0x3C, 0x64];
        assert!(matches!(
            parse(0, track),
            Err(MidiFileError::InvalidChunkData(_))
        ));
    }

    #[test]
    fn zero_smpte_ticks_per_frame_is_rejected() {
        // -25 fps, 0 ticks/frame: a tick would be an infinite number of seconds.
        let division = i16::from_be_bytes([0xE7, 0x00]);
        let track = vec![0x00, 0x90, 0x3C, 0x64];
        assert!(matches!(
            parse(division, track),
            Err(MidiFileError::InvalidChunkData(_))
        ));
    }

    #[test]
    fn smpte_division_uses_fixed_seconds_per_tick_and_ignores_tempo() {
        // -25 fps, 40 ticks/frame => 1000 ticks/sec => 1 ms/tick.
        let division = i16::from_be_bytes([0xE7, 0x28]);

        let track = vec![
            0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, // Tempo meta event (must be ignored)
            0x87, 0x68, 0x90, 0x3C, 0x64, // Note On 1000 ticks later
        ];

        let midi_file = parse(division, track).unwrap();

        assert_eq!(midi_file.times[0], 1.0);
    }
}
