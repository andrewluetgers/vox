//! Local Kokoro ONNX provider — the bundled default. Owns the loaded model.

use super::{Availability, Provider, SynthReq, WordEnd};
use anyhow::Result;
use kokoros::tts::koko::TTSKoko;

pub const VOICE_NAMES: &[&str] = &[
    // British male / female
    "bm_george", "bm_lewis", "bm_daniel", "bm_fable", "bf_emma", "bf_isabella",
    // American male / female
    "am_adam", "am_michael", "af_heart", "af_bella", "af_nicole", "af_sarah",
];

/// espeak voice from the vox voice prefix: bm_/bf_ British, am_/af_ American.
/// Note: this espeak-ng build has no bare "en-gb"; RP is the British voice.
pub fn lang_for(voice: &str) -> &'static str {
    if voice.starts_with('b') {
        "en-gb-x-rp"
    } else {
        "en-us"
    }
}

pub struct Kokoro {
    pub tts: TTSKoko,
}

impl Provider for Kokoro {
    fn name(&self) -> &'static str {
        "kokoro"
    }

    fn default_model(&self) -> &'static str {
        "v1.0"
    }

    fn voices(&self) -> Vec<String> {
        VOICE_NAMES.iter().map(|v| v.to_string()).collect()
    }

    fn availability(&self) -> Availability {
        Availability::Ready
    }

    fn synth(
        &self,
        req: &SynthReq,
        on_audio: &mut dyn FnMut(&[f32]) -> bool,
    ) -> Result<Option<Vec<WordEnd>>> {
        let mut cancelled = false;
        let result = self.tts.tts_raw_audio_streaming(
            req.text,
            lang_for(req.voice),
            req.voice,
            req.speed,
            None,
            None,
            None,
            None,
            |audio| {
                if !on_audio(&audio) {
                    cancelled = true;
                    return Err("cancelled".into());
                }
                Ok(())
            },
        );
        match result {
            Ok(_) => Ok(None), // no engine alignment; caller estimates timing
            Err(_) if cancelled => Ok(None),
            Err(e) => Err(anyhow::anyhow!("kokoro synthesis failed: {e}")),
        }
    }
}
