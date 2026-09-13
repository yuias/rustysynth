#![allow(dead_code)]

use std::cmp;
use std::sync::Arc;

use crate::midifile::Message;
use crate::midifile::MidiFile;
use crate::synthesizer::Synthesizer;

/// An instance of the MIDI file sequencer.
#[derive(Debug)]
#[non_exhaustive]
pub struct MidiFileSequencer {
    synthesizer: Synthesizer,

    speed: f64,

    midi_file: Option<Arc<MidiFile>>,
    play_loop: bool,

    block_wrote: usize,

    current_time: f64,
    msg_index: usize,
    loop_index: usize,
}

impl MidiFileSequencer {
    /// Initializes a new instance of the sequencer.
    ///
    /// # Arguments
    ///
    /// * `synthesizer` - The synthesizer to be handled by the sequencer.
    pub fn new(synthesizer: Synthesizer) -> Self {
        Self {
            synthesizer,
            speed: 1.0,
            midi_file: None,
            play_loop: false,
            block_wrote: 0,
            current_time: 0.0,
            msg_index: 0,
            loop_index: 0,
        }
    }

    /// Plays the MIDI file.
    ///
    /// # Arguments
    ///
    /// * `midi_file` - The MIDI file to be played.
    /// * `play_loop` - If `true`, the MIDI file loops after reaching the end.
    pub fn play(&mut self, midi_file: &Arc<MidiFile>, play_loop: bool) {
        self.midi_file = Some(Arc::clone(midi_file));
        self.play_loop = play_loop;

        self.block_wrote = self.synthesizer.block_size;

        self.current_time = 0.0;
        self.msg_index = 0;
        self.loop_index = 0;

        self.synthesizer.reset()
    }

    /// Stops playing.
    pub fn stop(&mut self) {
        self.midi_file = None;
        self.synthesizer.reset();
    }

    /// Renders the waveform.
    ///
    /// # Arguments
    ///
    /// * `left` - The buffer of the left channel to store the rendered waveform.
    /// * `right` - The buffer of the right channel to store the rendered waveform.
    ///
    /// # Remarks
    ///
    /// The output buffers for the left and right must be the same length.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        if left.len() != right.len() {
            panic!("The output buffers for the left and right must be the same length.");
        }

        let left_length = left.len();
        let mut wrote: usize = 0;
        while wrote < left_length {
            if self.block_wrote == self.synthesizer.block_size {
                self.process_events();
                self.block_wrote = 0;
                self.current_time += self.speed * self.synthesizer.block_size as f64
                    / self.synthesizer.sample_rate as f64;
            }

            let src_rem = self.synthesizer.block_size - self.block_wrote;
            let dst_rem = left_length - wrote;
            let rem = cmp::min(src_rem, dst_rem);

            self.synthesizer.render(
                &mut left[wrote..wrote + rem],
                &mut right[wrote..wrote + rem],
            );

            self.block_wrote += rem;
            wrote += rem;
        }
    }

    fn process_events(&mut self) {
        let midi_file = match self.midi_file.as_ref() {
            Some(value) => value,
            None => return,
        };

        while self.msg_index < midi_file.messages.len() {
            let time = midi_file.times[self.msg_index];
            let msg = midi_file.messages[self.msg_index];

            if time <= self.current_time {
                match msg {
                    Message::Normal {
                        status,
                        data1,
                        data2,
                    } => {
                        let channel = status & 0x0F;
                        let command = status & 0xF0;
                        self.synthesizer.process_midi_message(
                            channel as i32,
                            command as i32,
                            data1 as i32,
                            data2 as i32,
                        );
                    }
                    Message::SysEx { bytes } => {
                        let index = Message::sysex_index(bytes);
                        self.synthesizer.process_sysex(&midi_file.sysex_data[index]);
                    }
                    Message::LoopStart if self.play_loop => self.loop_index = self.msg_index,
                    Message::LoopEnd if self.play_loop => {
                        self.current_time = midi_file.times[self.loop_index];
                        self.msg_index = self.loop_index;
                        self.synthesizer.note_off_all(false);
                    }
                    _ => (),
                }
                self.msg_index += 1;
            } else {
                break;
            }
        }

        if self.msg_index == midi_file.messages.len() && self.play_loop {
            self.current_time = midi_file.times[self.loop_index];
            self.msg_index = self.loop_index;
            self.synthesizer.note_off_all(false);
        }
    }

    /// Gets the synthesizer handled by the sequencer.
    pub fn get_synthesizer(&self) -> &Synthesizer {
        &self.synthesizer
    }

    /// Gets the currently playing MIDI file.
    pub fn get_midi_file(&self) -> Option<&MidiFile> {
        match &self.midi_file {
            None => None,
            Some(value) => Some(value),
        }
    }

    /// Gets the current playback position in seconds.
    pub fn get_position(&self) -> f64 {
        self.current_time
    }

    /// Gets a value that indicates whether the current playback position is at the end of the sequence.
    ///
    /// # Remarks
    ///
    /// If the `play` method has not yet been called, this value will be `true`.
    /// This value will never be `true` if loop playback is enabled.
    pub fn end_of_sequence(&self) -> bool {
        match &self.midi_file {
            None => true,
            Some(value) => self.msg_index == value.messages.len(),
        }
    }

    /// Gets the current playback speed.
    ///
    /// # Remarks
    ///
    /// The default value is 1.
    /// The tempo will be multiplied by this value during playback.
    pub fn get_speed(&self) -> f64 {
        self.speed
    }

    /// Sets the playback speed.
    ///
    /// # Remarks
    ///
    /// The value must be non-negative.
    pub fn set_speed(&mut self, value: f64) {
        if value < 0.0 {
            panic!("The playback speed must be a non-negative value.");
        }

        self.speed = value;
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::synthesizer_settings::SynthesizerSettings;
    use crate::test_util::sine_soundfont;

    /// Builds a single-track, format-0 SMF with a fixed division and the given
    /// track body. An End of Track meta event is appended automatically.
    fn build_midi_file(track_data: &[u8]) -> Vec<u8> {
        let mut track_data = track_data.to_vec();
        track_data.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MThd");
        bytes.extend_from_slice(&6_i32.to_be_bytes());
        bytes.extend_from_slice(&0_i16.to_be_bytes());
        bytes.extend_from_slice(&1_i16.to_be_bytes());
        bytes.extend_from_slice(&480_i16.to_be_bytes());
        bytes.extend_from_slice(b"MTrk");
        bytes.extend_from_slice(&(track_data.len() as i32).to_be_bytes());
        bytes.extend_from_slice(&track_data);
        bytes
    }

    #[test]
    fn gs_scale_tuning_sysex_reaches_synthesizer() {
        // GS DT1: Scale Tuning for part 1 (-> MIDI channel 0), 12 bytes at +10 cents each.
        let mut payload = vec![0x41, 0x10, 0x42, 0x12, 0x40, 0x11, 0x40];
        payload.extend(std::iter::repeat(0x4A_u8).take(12));
        payload.push(0x00); // checksum byte (unchecked by the synthesizer)

        let mut track = vec![0x00, 0xF0, (payload.len() + 1) as u8];
        track.extend_from_slice(&payload);
        track.push(0xF7);

        let midi_file = Arc::new(MidiFile::new(&mut Cursor::new(build_midi_file(&track))).unwrap());

        let settings = SynthesizerSettings::new(44100);
        let synthesizer = Synthesizer::new(&sine_soundfont(), &settings).unwrap();
        let mut sequencer = MidiFileSequencer::new(synthesizer);

        sequencer.play(&midi_file, false);

        let block_size = sequencer.get_synthesizer().get_block_size();
        let mut left = vec![0_f32; block_size];
        let mut right = vec![0_f32; block_size];
        sequencer.render(&mut left, &mut right);

        assert_eq!(
            sequencer.get_synthesizer().get_scale_tuning(0),
            Some(&[10.0_f32; 12])
        );
        // Unaffected channel keeps the default (untouched) tuning.
        assert_eq!(
            sequencer.get_synthesizer().get_scale_tuning(1),
            Some(&[0.0_f32; 12])
        );
    }
}
