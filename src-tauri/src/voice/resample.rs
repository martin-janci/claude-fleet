//! Device audio → the relay's PCM: average the channels, resample to 16 kHz
//! by linear interpolation (plenty for speech), S16LE.

pub const OUT_RATE: u32 = 16_000;

pub struct Converter {
    /// Input frames per output frame.
    step: f64,
    /// Position of the next output frame, in input frames of the current chunk.
    pos: f64,
    /// Last input frame of the previous chunk.
    prev: f32,
    channels: usize,
}

impl Converter {
    pub fn new(in_rate: u32, channels: u16) -> Self {
        Self {
            step: in_rate as f64 / OUT_RATE as f64,
            pos: 0.0,
            prev: 0.0,
            channels: channels.max(1) as usize,
        }
    }

    /// Interleaved f32 in → S16LE mono 16 kHz bytes out.
    pub fn push(&mut self, input: &[f32]) -> Vec<u8> {
        let mono: Vec<f32> = input
            .chunks(self.channels)
            .map(|f| f.iter().sum::<f32>() / f.len() as f32)
            .collect();
        let mut out = Vec::with_capacity((mono.len() as f64 / self.step) as usize * 2 + 2);
        // Frame -1 is `prev`, so interpolation spans the chunk boundary.
        let at = |i: isize| if i < 0 { self.prev } else { mono[i as usize] };
        while self.pos < mono.len() as f64 - 1.0 + 1e-9 {
            let i = self.pos.floor() as isize;
            let frac = (self.pos - i as f64) as f32;
            let a = at(i);
            let b = if (i + 1) < mono.len() as isize {
                at(i + 1)
            } else {
                a
            };
            let v = (a + (b - a) * frac).clamp(-1.0, 1.0);
            out.extend_from_slice(&((v * 32767.0) as i16).to_le_bytes());
            self.pos += self.step;
        }
        if let Some(&last) = mono.last() {
            self.prev = last;
        }
        self.pos -= mono.len() as f64;
        out
    }
}

/// How loud one chunk of the relay's PCM is, 0.0–1.0: the RMS of its S16LE
/// samples on a square-root curve, so speech reads well above the noise
/// floor. The UI's Sonar follows it while the microphone records (redesign
/// 5.14).
pub fn level(pcm: &[u8]) -> f32 {
    let n = pcm.len() / 2;
    if n == 0 {
        return 0.0;
    }
    let sum: f64 = pcm
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| {
            let s = i16::from_le_bytes([b[0], b[1]]) as f64 / i16::MAX as f64;
            s * s
        })
        .sum();
    ((sum / n as f64).sqrt().sqrt() as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn samples(b: &[u8]) -> Vec<i16> {
        b.chunks(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect()
    }

    #[test]
    fn level_is_zero_for_silence_and_rises_with_loudness() {
        let pcm = |v: i16| v.to_le_bytes().repeat(100);
        assert_eq!(level(&[]), 0.0);
        assert_eq!(level(&pcm(0)), 0.0);
        let quiet = level(&pcm(300));
        let loud = level(&pcm(12_000));
        assert!(quiet > 0.0 && quiet < loud && loud < 1.0, "{quiet} {loud}");
        assert!((level(&pcm(i16::MAX)) - 1.0).abs() < 1e-6);
        assert!((level(&pcm(i16::MIN)) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn mono_16k_passes_through() {
        let mut c = Converter::new(16_000, 1);
        assert_eq!(
            samples(&c.push(&[0.0, 0.5, -0.5, 1.0])),
            vec![0, 16383, -16383, 32767]
        );
    }
    #[test]
    fn stereo_is_averaged() {
        let mut c = Converter::new(16_000, 2);
        assert_eq!(
            samples(&c.push(&[1.0, 0.0, -1.0, -1.0])),
            vec![16383, -32767]
        );
    }
    #[test]
    fn a_48k_second_becomes_a_16k_second_across_chunk_boundaries() {
        let mut c = Converter::new(48_000, 1);
        let mut n = 0;
        for _ in 0..10 {
            n += c.push(&vec![0.25; 4_800]).len() / 2;
        }
        assert!((15_999..=16_001).contains(&n), "{n}");
    }
    #[test]
    fn chunking_does_not_change_the_output() {
        // A 440 Hz stereo sine at 44.1 kHz: a ratio that is not a whole number.
        let input: Vec<f32> = (0..44_100)
            .flat_map(|i| {
                let v = (i as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin() * 0.8;
                [v, v]
            })
            .collect();
        let whole = samples(&Converter::new(44_100, 2).push(&input));
        let mut c = Converter::new(44_100, 2);
        let mut chunked = Vec::new();
        // Odd frame counts (×2 samples, so no frame is split).
        for part in input.chunks(2 * 97).flat_map(|p| p.chunks(2 * 13)) {
            chunked.extend(samples(&c.push(part)));
        }
        assert!(
            (whole.len() as i64 - chunked.len() as i64).abs() <= 1,
            "{} vs {}",
            whole.len(),
            chunked.len()
        );
        for (i, (a, b)) in whole.iter().zip(&chunked).enumerate() {
            assert!((*a as i32 - *b as i32).abs() <= 1, "sample {i}: {a} vs {b}");
        }
    }
    #[test]
    fn out_of_range_input_is_clamped() {
        let mut c = Converter::new(16_000, 1);
        assert_eq!(samples(&c.push(&[2.0, -2.0])), vec![32767, -32767]);
    }
}
