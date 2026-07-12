//! vox — speak text aloud with Kokoro TTS. Local, fast, streaming.

use anyhow::{bail, Context, Result};
use clap::Parser;
use kokoro_tts::{KokoroTts, Voice};
use rodio::{buffer::SamplesBuffer, OutputStream, Sink};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

const SAMPLE_RATE: u32 = 24000;
const RELEASE_BASE: &str = "https://github.com/mzdk100/kokoro/releases/download/V1.0";
// fp32 over int8: ~2x faster on Apple Silicon CPUs despite the bigger download
const MODEL_FILE: &str = "kokoro-v1.0.onnx";
const VOICES_FILE: &str = "voices.bin";
const MAX_CHUNK_CHARS: usize = 400;

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
    #[arg(short, long, default_value = "bm_george")]
    voice: String,
    /// Speech speed
    #[arg(short, long, default_value_t = 1.0)]
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
    /// Download model files (~115 MB) and exit
    #[arg(long)]
    setup: bool,
}

fn voice_for(name: &str, speed: f32) -> Result<Voice> {
    Ok(match name {
        "bm_george" => Voice::BmGeorge(speed),
        "bm_lewis" => Voice::BmLewis(speed),
        "bm_daniel" => Voice::BmDaniel(speed),
        "bm_fable" => Voice::BmFable(speed),
        "bf_emma" => Voice::BfEmma(speed),
        "bf_isabella" => Voice::BfIsabella(speed),
        "am_adam" => Voice::AmAdam(speed),
        "am_michael" => Voice::AmMichael(speed),
        "af_heart" => Voice::AfHeart(speed),
        "af_bella" => Voice::AfBella(speed),
        "af_nicole" => Voice::AfNicole(speed),
        "af_sarah" => Voice::AfSarah(speed),
        _ => bail!("unknown voice '{name}' (try --list-voices)"),
    })
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
    let model = dir.join(model_file);
    let voices = dir.join(VOICES_FILE);
    if !model.exists() {
        download(&format!("{RELEASE_BASE}/{MODEL_FILE}"), &model)?;
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

/// Split on sentence boundaries into chunks the model handles well.
/// The first chunk is a single sentence so audio starts as soon as possible.
fn chunk_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut sentence = String::new();
    for ch in text.chars() {
        sentence.push(ch);
        if matches!(ch, '.' | '!' | '?' | ';' | ':' | '\n') {
            flush_sentence(&mut chunks, &mut current, &mut sentence);
            if chunks.is_empty() && !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
        }
    }
    flush_sentence(&mut chunks, &mut current, &mut sentence);
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }
    chunks
}

fn flush_sentence(chunks: &mut Vec<String>, current: &mut String, sentence: &mut String) {
    let s = sentence.trim();
    if !s.is_empty() {
        if !current.is_empty() && current.len() + s.len() + 1 > MAX_CHUNK_CHARS {
            chunks.push(current.trim().to_string());
            current.clear();
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(s);
    }
    sentence.clear();
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

    let (model_path, voices_path) = ensure_models()?;
    if args.setup {
        eprintln!("Models ready in {}", cache_dir().display());
        return Ok(());
    }

    let text = read_text(&args)?;
    let voice = voice_for(&args.voice, args.speed)?;

    let t0 = Instant::now();
    let tts = KokoroTts::new(&model_path, &voices_path)
        .await
        .context("failed to load model")?;
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
    for chunk in chunk_text(&text) {
        let (audio, _took) = tts.synth(&chunk, voice.clone()).await?;
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
    }

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
