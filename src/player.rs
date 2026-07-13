//! Seekable playback over a growing sample buffer.
//!
//! rodio's Sink is an append-only queue with no seeking, so vox plays through
//! a cursor-based Source instead: synthesis appends samples to a shared
//! buffer while the cursor moves through it at a controllable rate and
//! direction. Skipping moves the cursor; scrubbing raises the rate to 3x
//! (negative for reverse); speed changes adjust the step size (tape-style,
//! pitch shifts with rate).

use rodio::Source;
use std::sync::{
    atomic::{AtomicBool, AtomicI8, AtomicU32, AtomicU64, Ordering},
    Arc, RwLock,
};
use std::time::Duration;

pub const SAMPLE_RATE: u32 = 24000;
pub const SCRUB_RATE: f32 = 3.0;
/// keep skips/scrubs this far behind live synthesis (samples)
const FRONTIER_MARGIN: f64 = SAMPLE_RATE as f64 / 2.0;

#[derive(Clone)]
pub struct Player {
    pub buf: Arc<RwLock<Vec<f32>>>,
    /// cursor position in samples, stored as f64 bits for fractional stepping
    pos: Arc<AtomicU64>,
    /// playback rate, stored as f32 bits (1.0 = normal)
    rate: Arc<AtomicU32>,
    /// 0 = normal, +1 = scrub forward, -1 = scrub backward
    scrub: Arc<AtomicI8>,
    /// set once synthesis has finished appending
    pub synth_done: Arc<AtomicBool>,
    /// bumped on every seek so the source can reset its stretch state
    seek_gen: Arc<AtomicU64>,
}

impl Player {
    pub fn new() -> Self {
        Self {
            buf: Arc::new(RwLock::new(Vec::new())),
            pos: Arc::new(AtomicU64::new(0f64.to_bits())),
            rate: Arc::new(AtomicU32::new(1f32.to_bits())),
            scrub: Arc::new(AtomicI8::new(0)),
            synth_done: Arc::new(AtomicBool::new(false)),
            seek_gen: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn append(&self, samples: &[f32]) {
        self.buf.write().unwrap().extend_from_slice(samples);
    }

    pub fn len(&self) -> usize {
        self.buf.read().unwrap().len()
    }

    pub fn pos(&self) -> f64 {
        f64::from_bits(self.pos.load(Ordering::Relaxed))
    }

    fn set_pos(&self, p: f64) {
        self.pos.store(p.to_bits(), Ordering::Relaxed);
    }

    /// How far forward the cursor may go: the end once synthesis is done,
    /// otherwise just behind the synthesis frontier so skips never land in
    /// silence that hasn't been generated yet.
    fn max_pos(&self) -> f64 {
        let len = self.len() as f64;
        if self.synth_done.load(Ordering::Relaxed) {
            len
        } else {
            (len - FRONTIER_MARGIN).max(0.0)
        }
    }

    /// Move the cursor by `secs` (negative = back), clamped to generated audio.
    pub fn skip(&self, secs: f32) {
        let p = (self.pos() + secs as f64 * SAMPLE_RATE as f64).clamp(0.0, self.max_pos());
        self.set_pos(p);
        self.seek_gen.fetch_add(1, Ordering::Relaxed);
    }

    /// Jump to the end of generated audio (used to stop the current read).
    pub fn jump_to_end(&self) {
        self.set_pos(self.len() as f64);
        self.seek_gen.fetch_add(1, Ordering::Relaxed);
    }

    pub fn rate(&self) -> f32 {
        f32::from_bits(self.rate.load(Ordering::Relaxed))
    }

    /// Adjust the playback rate by `delta`, clamped to 0.25–3.0. Returns the new rate.
    pub fn adjust_rate(&self, delta: f32) -> f32 {
        let r = (self.rate() + delta).clamp(0.25, 3.0);
        self.rate.store(r.to_bits(), Ordering::Relaxed);
        r
    }

    pub fn set_scrub(&self, dir: i8) {
        self.scrub.store(dir, Ordering::Relaxed);
    }

    pub fn scrubbing(&self) -> i8 {
        self.scrub.load(Ordering::Relaxed)
    }

    pub fn source(&self) -> CursorSource {
        CursorSource {
            player: self.clone(),
            stretch: signalsmith_stretch::Stretch::preset_default(1, SAMPLE_RATE),
            out: std::collections::VecDeque::new(),
            seen_gen: 0,
            flushed: false,
        }
    }
}

pub struct CursorSource {
    player: Player,
    /// pitch-preserving time stretcher, used when rate != 1.0
    stretch: signalsmith_stretch::Stretch,
    /// stretched samples waiting to be played
    out: std::collections::VecDeque<f32>,
    seen_gen: u64,
    flushed: bool,
}

/// stretched output is produced in blocks of this many samples (~43 ms)
const OUT_BLOCK: usize = 1024;

impl CursorSource {
    /// Plain per-sample stepping: normal 1x playback and scrubbing.
    /// Pitch follows rate here, which is what you want for a 3x scrub cue.
    fn step_plain(&mut self) -> Option<f32> {
        let p = &self.player;
        let buf = p.buf.read().unwrap();
        let len = buf.len() as f64;
        let scrub = p.scrubbing();
        let step = if scrub != 0 {
            SCRUB_RATE as f64 * scrub as f64
        } else {
            1.0
        };
        let mut pos = p.pos() + step;

        if pos < 0.0 {
            // hit the start while scrubbing backward: hold at 0
            pos = 0.0;
            p.set_scrub(0);
        }
        if scrub > 0 && pos >= p.max_pos() {
            // scrubbing caught up with synthesis: drop back to normal playback
            p.set_scrub(0);
            pos = p.max_pos();
        }
        if pos >= len {
            if p.synth_done.load(Ordering::Relaxed) {
                return None; // played past the end of finished audio
            }
            // playback caught up with synthesis: hold until more arrives
            p.set_pos(len);
            return Some(0.0);
        }
        p.set_pos(pos);
        Some(buf[pos as usize])
    }

    /// Pitch-preserving playback through the time stretcher (rate != 1.0):
    /// consume `rate * OUT_BLOCK` input samples per OUT_BLOCK of output.
    fn step_stretched(&mut self, rate: f32) -> Option<f32> {
        if let Some(s) = self.out.pop_front() {
            return Some(s);
        }
        let p = &self.player;
        let need = ((OUT_BLOCK as f64) * rate as f64).round() as usize;
        let buf = p.buf.read().unwrap();
        let len = buf.len();
        let pos = p.pos() as usize;
        let synth_done = p.synth_done.load(Ordering::Relaxed);

        if pos + need > len {
            if !synth_done {
                return Some(0.0); // hold at the frontier until more audio arrives
            }
            if pos < len {
                // final partial block
                let input: Vec<f32> = buf[pos..len].to_vec();
                drop(buf);
                p.set_pos(len as f64);
                let out_n = ((input.len() as f64 / rate as f64).round() as usize).max(1);
                let mut out = vec![0.0; out_n];
                self.stretch.process(&input, &mut out);
                self.out.extend(out);
                return self.out.pop_front();
            }
            if !self.flushed {
                // drain the stretcher's internal latency
                self.flushed = true;
                let mut out = vec![0.0; OUT_BLOCK];
                self.stretch.flush(&mut out);
                self.out.extend(out);
                return self.out.pop_front();
            }
            return None;
        }

        let input: Vec<f32> = buf[pos..pos + need].to_vec();
        drop(buf);
        p.set_pos((pos + need) as f64);
        let mut out = vec![0.0; OUT_BLOCK];
        self.stretch.process(&input, &mut out);
        self.out.extend(out);
        self.out.pop_front()
    }
}

impl Iterator for CursorSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let p = &self.player;
        // a seek happened: drop queued stretched audio so we don't smear across the jump
        let gen = p.seek_gen.load(Ordering::Relaxed);
        if gen != self.seen_gen {
            self.seen_gen = gen;
            self.out.clear();
            self.stretch.reset();
        }
        let rate = p.rate();
        if p.scrubbing() != 0 || (rate - 1.0).abs() < 0.01 {
            if !self.out.is_empty() {
                self.out.clear();
                self.stretch.reset();
            }
            self.step_plain()
        } else {
            self.step_stretched(rate)
        }
    }
}

impl Source for CursorSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        1
    }
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player_with_secs(secs: u32) -> Player {
        let p = Player::new();
        let n = (secs * SAMPLE_RATE) as usize;
        let samples: Vec<f32> = (0..n).map(|i| (i as f32 * 0.001).sin() * 0.5).collect();
        p.append(&samples);
        p
    }

    #[test]
    fn skip_moves_cursor_and_clamps() {
        let p = player_with_secs(60);
        let mut src = p.source();
        for _ in 0..1000 {
            src.next().unwrap();
        }
        assert!((p.pos() - 1000.0).abs() < 2.0);

        p.skip(-15.0); // clamps to 0
        assert_eq!(p.pos(), 0.0);

        p.skip(20.0);
        assert!((p.pos() - 20.0 * SAMPLE_RATE as f64).abs() < 2.0);

        // forward skip is capped 0.5s behind the frontier while synthesizing
        p.skip(9999.0);
        let cap = 60.0 * SAMPLE_RATE as f64 - SAMPLE_RATE as f64 / 2.0;
        assert_eq!(p.pos(), cap);

        // and to the true end once synthesis is done
        p.synth_done.store(true, Ordering::SeqCst);
        p.skip(9999.0);
        assert_eq!(p.pos(), 60.0 * SAMPLE_RATE as f64);
    }

    #[test]
    fn playback_resumes_after_seek_back() {
        let p = player_with_secs(10);
        let mut src = p.source();
        for _ in 0..(5 * SAMPLE_RATE as usize) {
            src.next().unwrap();
        }
        p.skip(-4.0);
        let before = p.pos();
        let s = src.next().unwrap();
        assert!(s != 0.0, "should be reading real samples after seek back");
        assert!(p.pos() >= before);
    }

    #[test]
    fn stretched_playback_advances_at_rate_and_produces_audio() {
        let p = player_with_secs(30);
        p.synth_done.store(true, Ordering::SeqCst);
        p.adjust_rate(0.5); // 1.0 + 0.5 = 1.5x
        assert_eq!(p.rate(), 1.5);

        let mut src = p.source();
        let n_out = 4 * OUT_BLOCK;
        let mut nonzero = 0;
        for _ in 0..n_out {
            let s = src.next().expect("audio should continue");
            if s.abs() > 1e-6 {
                nonzero += 1;
            }
        }
        // consumed input ≈ output * rate
        let expected = n_out as f64 * 1.5;
        assert!(
            (p.pos() - expected).abs() < 2.0 * OUT_BLOCK as f64,
            "pos {} vs expected {expected}",
            p.pos()
        );
        // the stretcher has startup latency, but most of the block should be sound
        assert!(nonzero > n_out / 2, "only {nonzero}/{n_out} nonzero samples");
    }

    #[test]
    fn stretched_playback_ends_after_flush() {
        let p = player_with_secs(1);
        p.synth_done.store(true, Ordering::SeqCst);
        p.adjust_rate(1.0); // 2.0x
        let mut src = p.source();
        let mut count = 0usize;
        while src.next().is_some() {
            count += 1;
            assert!(count < 10 * SAMPLE_RATE as usize, "source never ended");
        }
        // ~1s of audio at 2x -> ~0.5s of output (plus flush tail)
        assert!(count > SAMPLE_RATE as usize / 4, "too little output: {count}");
        assert!(count < SAMPLE_RATE as usize, "too much output: {count}");
    }
}