#![allow(dead_code)]

use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

fn load(name: &str) -> Arc<SoundFont> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.push(name);
    let mut file = File::open(&path).unwrap();
    Arc::new(SoundFont::new(&mut file).unwrap())
}

// Plays notes on several presets while moving controllers that feed SoundFont modulators,
// and checks that rendering stays finite and audible.
fn render_with_controllers(sound_font: &Arc<SoundFont>, programs: &[(i32, i32, i32)]) {
    let settings = SynthesizerSettings::new(44100);
    let mut synthesizer = Synthesizer::new(sound_font, &settings).unwrap();
    let mut left = vec![0_f32; 4410];
    let mut right = vec![0_f32; 4410];
    let mut energy = 0_f64;

    for (i, &(channel, bank, program)) in programs.iter().enumerate() {
        synthesizer.process_midi_message(channel, 0xB0, 0, bank);
        synthesizer.process_midi_message(channel, 0xC0, program, 0);
        synthesizer.note_on(channel, 36 + 7 * i as i32, 30 + 12 * i as i32);
        synthesizer.note_on(channel, 60 + i as i32, 100);
    }

    let controller_steps: [(i32, i32, i32); 6] = [
        (0xB0, 1, 127),
        (0xB0, 2, 90),
        (0xD0, 100, 0),
        (0xA0, 60, 120),
        (0xE0, 0, 127),
        (0xE0, 127, 0),
    ];
    for &(command, data1, data2) in controller_steps.iter() {
        for &(channel, _, _) in programs {
            synthesizer.process_midi_message(channel, command, data1, data2);
        }
        synthesizer.render(&mut left, &mut right);
        for (&l, &r) in left.iter().zip(right.iter()) {
            assert!(l.is_finite() && r.is_finite());
            energy += (l * l + r * r) as f64;
        }
    }

    assert!(energy > 0.0);
}

#[test]
fn timgm6mb_renders_with_modulators() {
    let sound_font = load("TimGM6mb.sf2");
    render_with_controllers(&sound_font, &[(0, 0, 0), (1, 0, 48), (2, 0, 73), (9, 0, 0)]);
}

#[test]
fn musescore_renders_with_modulators() {
    let sound_font = load("GeneralUser GS MuseScore v1.442.sf2");
    render_with_controllers(
        &sound_font,
        &[(0, 0, 0), (1, 0, 48), (2, 8, 118), (3, 120, 25), (9, 25, 0)],
    );
}
