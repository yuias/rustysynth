#![allow(dead_code)]

use std::f64::consts;

/// Number of modulated delay voices per channel.
const NUM_VOICES: usize = 3;

/// 3-voice stereo chorus with optional feedback.
///
/// Each channel (L/R) has 3 modulated delay taps at 120-degree phase spacing.
/// L voices: 0°, 120°, 240°; R voices: 90°, 210°, 330° (90° stereo offset).
/// Feedback path enables FB Chorus and Flanger presets.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct Chorus {
    buffer_l: Vec<f32>,
    buffer_r: Vec<f32>,

    delay_table: Vec<f32>,

    buffer_index: usize,

    dt_indices_l: [usize; NUM_VOICES],
    dt_indices_r: [usize; NUM_VOICES],

    feedback: f32,
    prev_out_l: f32,
    prev_out_r: f32,

    sample_rate: i32,
}

impl Chorus {
    /// Chorus type presets: (delay_s, depth_s, rate_hz, feedback).
    const PRESETS: [(f64, f64, f64, f64); 6] = [
        (0.006, 0.0015, 0.5, 0.0), // Type 0: Chorus 1 (light)
        (0.008, 0.0025, 0.6, 0.0), // Type 1: Chorus 2 (medium)
        (0.010, 0.0035, 0.8, 0.0), // Type 2: Chorus 3 (deep)
        (0.012, 0.0050, 1.0, 0.2), // Type 3: Chorus 4 (rich)
        (0.008, 0.0030, 0.4, 0.5), // Type 4: FB Chorus
        (0.002, 0.0008, 0.2, 0.7), // Type 5: Flanger
    ];

    pub(crate) fn new(sample_rate: i32, delay: f64, depth: f64, frequency: f64) -> Self {
        let mut chorus = Self {
            buffer_l: Vec::new(),
            buffer_r: Vec::new(),
            delay_table: Vec::new(),
            buffer_index: 0,
            dt_indices_l: [0; NUM_VOICES],
            dt_indices_r: [0; NUM_VOICES],
            feedback: 0.0,
            prev_out_l: 0.0,
            prev_out_r: 0.0,
            sample_rate,
        };
        chorus.set_params(sample_rate, delay, depth, frequency);
        chorus
    }

    pub(crate) fn process(
        &mut self,
        input_left: &[f32],
        input_right: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
    ) {
        let buffer_length = self.buffer_l.len();
        let delay_table_length = self.delay_table.len();
        let output_length = output_left.len();
        let inv_voices: f64 = 1.0 / NUM_VOICES as f64;

        for t in 0..output_length {
            // Sum 3 voices for L channel
            let mut sum_l = 0.0_f64;
            for v in 0..NUM_VOICES {
                let delay = self.delay_table[self.dt_indices_l[v]] as f64;
                let mut position = self.buffer_index as f64 - delay;
                if position < 0.0 {
                    position += buffer_length as f64;
                }

                let index1 = position as usize;
                let mut index2 = index1 + 1;
                if index2 == buffer_length {
                    index2 = 0;
                }

                let x1 = self.buffer_l[index1] as f64;
                let x2 = self.buffer_l[index2] as f64;
                let a = position - index1 as f64;
                sum_l += x1 + a * (x2 - x1);

                self.dt_indices_l[v] += 1;
                if self.dt_indices_l[v] == delay_table_length {
                    self.dt_indices_l[v] = 0;
                }
            }
            output_left[t] = (sum_l * inv_voices) as f32;

            // Sum 3 voices for R channel
            let mut sum_r = 0.0_f64;
            for v in 0..NUM_VOICES {
                let delay = self.delay_table[self.dt_indices_r[v]] as f64;
                let mut position = self.buffer_index as f64 - delay;
                if position < 0.0 {
                    position += buffer_length as f64;
                }

                let index1 = position as usize;
                let mut index2 = index1 + 1;
                if index2 == buffer_length {
                    index2 = 0;
                }

                let x1 = self.buffer_r[index1] as f64;
                let x2 = self.buffer_r[index2] as f64;
                let a = position - index1 as f64;
                sum_r += x1 + a * (x2 - x1);

                self.dt_indices_r[v] += 1;
                if self.dt_indices_r[v] == delay_table_length {
                    self.dt_indices_r[v] = 0;
                }
            }
            output_right[t] = (sum_r * inv_voices) as f32;

            // Write input + feedback to delay buffer
            self.buffer_l[self.buffer_index] = input_left[t] + self.feedback * self.prev_out_l;
            self.buffer_r[self.buffer_index] = input_right[t] + self.feedback * self.prev_out_r;
            self.prev_out_l = output_left[t];
            self.prev_out_r = output_right[t];

            self.buffer_index += 1;
            if self.buffer_index == buffer_length {
                self.buffer_index = 0;
            }
        }
    }

    pub(crate) fn set_params(
        &mut self,
        sample_rate: i32,
        delay: f64,
        depth: f64,
        frequency: f64,
    ) {
        self.set_params_with_feedback(sample_rate, delay, depth, frequency, self.feedback as f64);
    }

    pub(crate) fn set_params_with_feedback(
        &mut self,
        sample_rate: i32,
        delay: f64,
        depth: f64,
        frequency: f64,
        feedback: f64,
    ) {
        self.sample_rate = sample_rate;
        self.feedback = feedback as f32;

        self.buffer_l = vec![0_f32; ((sample_rate as f64) * (delay + depth)) as usize + 2];
        self.buffer_r = vec![0_f32; ((sample_rate as f64) * (delay + depth)) as usize + 2];

        self.delay_table = vec![0_f32; ((sample_rate as f64) / frequency).round() as usize];
        let delay_table_length = self.delay_table.len();
        for (t, entry) in self.delay_table.iter_mut().enumerate() {
            let phase = 2.0 * consts::PI * (t as f64) / (delay_table_length as f64);
            *entry = ((sample_rate as f64) * (delay + depth * phase.sin())) as f32;
        }

        self.buffer_index = 0;
        self.prev_out_l = 0.0;
        self.prev_out_r = 0.0;

        // 3 voices at 120° spacing; R offset 90° from L for stereo
        let third = delay_table_length / 3;
        let quarter = delay_table_length / 4;

        self.dt_indices_l = [0, third, 2 * third];
        self.dt_indices_r = [
            quarter % delay_table_length,
            (quarter + third) % delay_table_length,
            (quarter + 2 * third) % delay_table_length,
        ];
    }

    /// Select a GM chorus type preset (0-5).
    /// 0=Chorus1, 1=Chorus2, 2=Chorus3, 3=Chorus4, 4=FB Chorus, 5=Flanger.
    pub(crate) fn set_chorus_type(&mut self, type_id: i32) {
        if let Some(&(delay, depth, rate, fb)) = Self::PRESETS.get(type_id as usize) {
            self.set_params_with_feedback(self.sample_rate, delay, depth, rate, fb);
        }
    }

    /// Set the feedback gain directly (0.0 to <1.0).
    pub(crate) fn set_feedback(&mut self, value: f32) {
        self.feedback = value.clamp(0.0, 0.95);
    }

    pub(crate) fn mute(&mut self) {
        self.buffer_l.fill(0_f32);
        self.buffer_r.fill(0_f32);
        self.prev_out_l = 0.0;
        self.prev_out_r = 0.0;
    }
}
