//! vox — speak text aloud with Kokoro TTS. Local, fast, streaming.

use anyhow::{bail, Context, Result};
use clap::Parser;
use kokoros::tts::koko::TTSKoko;
use rodio::{buffer::SamplesBuffer, OutputStream, Sink};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

const SAMPLE_RATE: u32 = 24000;
const RELEASE_BASE: &str =
    "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0";
// fp32 over int8: ~2x faster on Apple Silicon CPUs despite the bigger download
const MODEL_FILE: &str = "kokoro-v1.0.onnx";
const VOICES_FILE: &str = "voices-v1.0.bin";

const VOICE_NAMES: &[&str] = &[
    // British male / female
    "bm_george", "bm_lewis", "bm_daniel", "bm_fable", "bf_emma", "bf_isabella",
    // American male / female
    "am_adam", "am_michael", "af_heart", "af_bella", "af_nicole", "af_sarah",
];

#[derive(Parser)]
#[command(name = "vox", version, about = "Read text aloud with Kokoro TTS (local, streaming)")]
struct Args {
    /// Text to speak (or use -f / -c / stdin)
    text: Option<String>,
    /// Read text from a file
    #[arg(short, long)]
    file: Option<PathBuf>,
    /// Read text from the clipboard
    #[arg(short, long)]
    clip: bool,
    /// Voice (see --list-voices)
    #[arg(short, long, env = "VOX_VOICE", default_value = "bm_george")]
    voice: String,
    /// Speech speed
    #[arg(short, long, env = "VOX_SPEED", default_value_t = 1.0)]
    speed: f32,
    /// Write audio to this wav file
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// Don't play, just save
    #[arg(long)]
    no_play: bool,
    /// Don't save a wav, just play
    #[arg(long)]
    no_save: bool,
    /// List available voices
    #[arg(long)]
    list_voices: bool,
    /// Download model files (~350 MB) and exit
    #[arg(long)]
    setup: bool,
}

/// Pronunciation fixes: lines of `word = respelling` in ~/.config/vox/lexicon.txt
/// (or $VOX_LEXICON). Matches whole words, case-insensitive, before synthesis.
fn load_lexicon() -> Vec<(String, String)> {
    let path = std::env::var_os("VOX_LEXICON")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_default()
                .join("vox")
                .join("lexicon.txt")
        });
    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (word, respell) = line.split_once('=')?;
            Some((word.trim().to_lowercase(), respell.trim().to_string()))
        })
        .collect()
}

fn apply_lexicon(text: &str, lex: &[(String, String)]) -> String {
    if lex.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    for ch in text.chars().chain(std::iter::once('\0')) {
        if ch.is_alphanumeric() || ch == '\'' {
            word.push(ch);
        } else {
            if !word.is_empty() {
                let lower = word.to_lowercase();
                match lex.iter().find(|(k, _)| *k == lower) {
                    Some((_, respell)) => out.push_str(respell),
                    None => out.push_str(&word),
                }
                word.clear();
            }
            if ch != '\0' {
                out.push(ch);
            }
        }
    }
    out
}

/// espeak voice from the vox voice prefix: bm_/bf_ British, am_/af_ American.
/// Note: this espeak-ng build has no bare "en-gb"; RP is the British voice.
fn lang_for(voice: &str) -> &'static str {
    if voice.starts_with('b') {
        "en-gb-x-rp"
    } else {
        "en-us"
    }
}

fn cache_dir() -> PathBuf {
    std::env::var_os("VOX_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::cache_dir().expect("no cache dir").join("vox"))
}

fn audio_dir() -> PathBuf {
    std::env::var_os("VOX_AUDIO_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::audio_dir().expect("no audio dir").join("vox"))
}

fn download(url: &str, dest: &Path) -> Result<()> {
    eprintln!("Downloading {url}...");
    let resp = ureq::get(url).call().context("download failed")?;
    let total: Option<u64> = resp.header("content-length").and_then(|v| v.parse().ok());
    let tmp = dest.with_extension("part");
    let mut file = fs::File::create(&tmp)?;
    let mut reader = resp.into_reader();
    let mut buf = [0u8; 1 << 16];
    let mut done: u64 = 0;
    let mut last_pct = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        if let Some(total) = total {
            let pct = (done * 100 / total) as u32;
            if pct >= last_pct + 10 {
                eprintln!("  {pct}% ({} / {} MB)", done >> 20, total >> 20);
                last_pct = pct;
            }
        }
    }
    fs::rename(&tmp, dest)?;
    Ok(())
}

fn ensure_models() -> Result<(PathBuf, PathBuf)> {
    let dir = cache_dir();
    fs::create_dir_all(&dir)?;
    let model_file =
        std::env::var("VOX_MODEL_FILE").unwrap_or_else(|_| MODEL_FILE.to_string());
    let model = dir.join(&model_file);
    let voices = dir.join(VOICES_FILE);
    if !model.exists() {
        download(&format!("{RELEASE_BASE}/{model_file}"), &model)?;
    }
    if !voices.exists() {
        download(&format!("{RELEASE_BASE}/{VOICES_FILE}"), &voices)?;
    }
    Ok((model, voices))
}

fn read_text(args: &Args) -> Result<String> {
    let text = if args.clip {
        let out = Command::new("pbpaste").output().context("pbpaste failed")?;
        String::from_utf8_lossy(&out.stdout).into_owned()
    } else if let Some(path) = &args.file {
        fs::read_to_string(path).with_context(|| format!("can't read {}", path.display()))?
    } else if let Some(text) = &args.text {
        text.clone()
    } else if !atty_stdin() {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        bail!("no text given (pass as argument, -f FILE, -c, or pipe stdin)");
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        bail!("input text is empty");
    }
    Ok(text)
}

fn atty_stdin() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

fn out_path(args: &Args, text: &str) -> Result<PathBuf> {
    if let Some(out) = &args.out {
        return Ok(out.clone());
    }
    let dir = audio_dir();
    fs::create_dir_all(&dir)?;
    let stamp = chrono_stamp();
    let slug: String = text
        .to_lowercase()
        .chars()
        .take(40)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    Ok(dir.join(format!("{stamp}-{slug}.wav")))
}

fn chrono_stamp() -> String {
    // seconds since epoch is enough to keep names unique and sortable
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    format!("{now}")
}

fn save_wav(path: &Path, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &s in samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.list_voices {
        for v in VOICE_NAMES {
            println!("{v}");
        }
        return Ok(());
    }

    // espeak-ng looks for its data via this env var (then cwd / exe dir);
    // we keep espeak-ng-data in the vox cache dir, installed by --setup docs.
    if std::env::var_os("PIPER_ESPEAKNG_DATA_DIRECTORY").is_none()
        && cache_dir().join("espeak-ng-data").exists()
    {
        std::env::set_var("PIPER_ESPEAKNG_DATA_DIRECTORY", cache_dir());
    }

    let (model_path, voices_path) = ensure_models()?;
    if args.setup {
        eprintln!("Models ready in {}", cache_dir().display());
        return Ok(());
    }

    let text = apply_lexicon(&read_text(&args)?, &load_lexicon());
    if !VOICE_NAMES.contains(&args.voice.as_str()) {
        bail!("unknown voice '{}' (try --list-voices)", args.voice);
    }

    let t0 = Instant::now();
    let tts = TTSKoko::new(
        model_path.to_str().context("bad model path")?,
        voices_path.to_str().context("bad voices path")?,
    )
    .await;
    let load = t0.elapsed();
    eprintln!("Model loaded in {:.2}s", load.as_secs_f32());

    let (_stream, sink) = if args.no_play {
        (None, None)
    } else {
        let (stream, handle) = OutputStream::try_default().context("no audio output device")?;
        let sink = Sink::try_new(&handle)?;
        (Some(stream), Some(sink))
    };

    let mut all_samples: Vec<f32> = Vec::new();
    let mut first_audio: Option<f32> = None;
    tts.tts_raw_audio_streaming(
        &text,
        lang_for(&args.voice),
        &args.voice,
        args.speed,
        None,
        None,
        None,
        None,
        |audio| {
            if first_audio.is_none() {
                let t = t0.elapsed().as_secs_f32();
                first_audio = Some(t);
                eprintln!(
                    "First audio at {t:.2}s (load {:.2}s + synth {:.2}s)",
                    load.as_secs_f32(),
                    t - load.as_secs_f32()
                );
            }
            if let Some(sink) = &sink {
                sink.append(SamplesBuffer::new(1, SAMPLE_RATE, audio.clone()));
            }
            all_samples.extend(audio);
            Ok(())
        },
    )
    .map_err(|e| anyhow::anyhow!("synthesis failed: {e}"))?;

    let total = t0.elapsed().as_secs_f32();
    let synth = total - load.as_secs_f32();
    let audio_secs = all_samples.len() as f32 / SAMPLE_RATE as f32;
    eprintln!(
        "Synthesized {audio_secs:.1}s of audio in {synth:.1}s ({:.1}x faster than realtime)",
        audio_secs / synth.max(0.001)
    );

    if !args.no_save {
        let out = out_path(&args, &text)?;
        save_wav(&out, &all_samples)?;
        eprintln!("Saved {}", out.display());
    }

    if let Some(sink) = &sink {
        sink.sleep_until_end();
    }
    Ok(())
}
