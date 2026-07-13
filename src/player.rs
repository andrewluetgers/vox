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
}

impl Player {
    pub fn new() -> Self {
        Self {
            buf: Arc::new(RwLock::new(Vec::new())),
            pos: Arc::new(AtomicU64::new(0f64.to_bits())),
            rate: Arc::new(AtomicU32::new(1f32.to_bits())),
            scrub: Arc::new(AtomicI8::new(0)),
            synth_done: Arc::new(AtomicBool::new(false)),
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

    /// Move the cursor by `secs` (negative = back), clamped to the buffer.
    pub fn skip(&self, secs: f32) {
        let len = self.len() as f64;
        let p = (self.pos() + secs as f64 * SAMPLE_RATE as f64).clamp(0.0, len);
        self.set_pos(p);
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
        }
    }
}

pub struct CursorSource {
    player: Player,
}

impl Iterator for CursorSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let p = &self.player;
        let buf = p.buf.read().unwrap();
        let len = buf.len() as f64;
        let scrub = p.scrubbing();
        let step = if scrub != 0 {
            SCRUB_RATE as f64 * scrub as f64
        } else {
            p.rate() as f64
        };
        let mut pos = p.pos() + step;

        if pos < 0.0 {
            // hit the start while scrubbing backward: hold at 0
            pos = 0.0;
            p.set_scrub(0);
        }
        if pos >= len {
            if p.synth_done.load(Ordering::Relaxed) {
                return None; // played (or scrubbed) past the end of finished audio
            }
            // ahead of synthesis: hold and emit silence until more arrives
            p.set_pos(len);
            return Some(0.0);
        }
        p.set_pos(pos);
        Some(buf[pos as usize])
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