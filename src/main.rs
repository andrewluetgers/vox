//! vox — speak text aloud with Kokoro TTS. Local, fast, streaming.

mod config;
mod player;
mod providers;
mod tui;

use anyhow::{bail, Context, Result};
use clap::Parser;
use kokoros::tts::koko::TTSKoko;
use player::SAMPLE_RATE;
use providers::kokoro::VOICE_NAMES;
use providers::{Availability, Provider, SynthReq, VoicePath};
use rodio::{OutputStream, Sink};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};
const RELEASE_BASE: &str =
    "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0";
// fp32 over int8: ~2x faster on Apple Silicon CPUs despite the bigger download
const MODEL_FILE: &str = "kokoro-v1.0.onnx";
const VOICES_FILE: &str = "voices-v1.0.bin";

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
    /// Voice (see --list-voices) [default: bm_george, or .vox.json]
    #[arg(short, long, env = "VOX_VOICE")]
    voice: Option<String>,
    /// Speech speed [default: 1.0, or .vox.json]
    #[arg(short, long, env = "VOX_SPEED")]
    speed: Option<f32>,
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
    /// With --list-voices: emit JSON (path, label, provider, ready)
    #[arg(long)]
    json: bool,
    /// Download model files (~350 MB) and exit
    #[arg(long)]
    setup: bool,
    /// Stop any vox currently speaking (from any terminal)
    #[arg(long)]
    stop: bool,
    /// Open the persistent reader UI (also the default with no arguments)
    #[arg(long)]
    ui: bool,
}

/// Kill every other running vox process (used by --stop).
fn stop_others() -> Result<()> {
    let me = std::process::id().to_string();
    let out = Command::new("pgrep").args(["-x", "vox"]).output()?;
    let mut killed = 0;
    for pid in String::from_utf8_lossy(&out.stdout).split_whitespace() {
        if pid != me {
            Command::new("kill").arg(pid).status()?;
            killed += 1;
        }
    }
    eprintln!(
        "{}",
        if killed > 0 { "Stopped." } else { "Nothing was speaking." }
    );
    Ok(())
}

fn fmt_time(samples: f64) -> String {
    let s = samples / player::SAMPLE_RATE as f64;
    format!("{}:{:02}", (s as u64) / 60, (s as u64) % 60)
}

fn status(player: &player::Player, msg: &str) {
    // raw mode: rewrite a single status line in place
    eprint!(
        "\r\x1b[K{msg}  [{} / {}]",
        fmt_time(player.pos()),
        fmt_time(player.len() as f64)
    );
}

/// While speaking interactively:
///   space          pause / resume
///   left / right   skip 15s (shift: 30s); hold to scrub at 3x (reverse plays backward)
///   up / down      playback speed +/- 0.25x
///   q / Esc / ^C   cancel
fn spawn_key_listener(
    sink: std::sync::Arc<Sink>,
    player: player::Player,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use crossterm::event::{poll, read, Event, KeyCode, KeyEventKind, KeyModifiers};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    // Terminal key auto-repeat: a held arrow arrives as rapid repeated events
    // (crossterm reports them as Press on most terminals, Repeat on some).
    // Two same-arrow events inside HOLD_WINDOW = holding -> scrub; when events
    // stop for SCRUB_TIMEOUT the hold has ended -> back to normal playback.
    const HOLD_WINDOW: Duration = Duration::from_millis(250);
    const SCRUB_TIMEOUT: Duration = Duration::from_millis(300);

    std::thread::spawn(move || {
        let _ = crossterm::terminal::enable_raw_mode();
        let mut last_arrow: Option<(KeyCode, Instant)> = None;
        let mut scrub_deadline = Instant::now();
        loop {
            if cancelled.load(Ordering::SeqCst) {
                break;
            }
            if player.scrubbing() != 0 && Instant::now() > scrub_deadline {
                player.set_scrub(0);
                status(&player, &format!("▶ {}x", player.rate()));
            }
            if !poll(Duration::from_millis(50)).unwrap_or(false) {
                continue;
            }
            let Ok(Event::Key(key)) = read() else {
                continue;
            };
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match key.code {
                KeyCode::Char(' ') => {
                    if sink.is_paused() {
                        sink.play();
                        status(&player, "▶ resumed");
                    } else {
                        sink.pause();
                        status(&player, "⏸ paused");
                    }
                }
                code @ (KeyCode::Left | KeyCode::Right) => {
                    let dir: i8 = if code == KeyCode::Left { -1 } else { 1 };
                    let now = Instant::now();
                    let holding = key.kind == KeyEventKind::Repeat
                        || matches!(last_arrow, Some((c, t)) if c == code && now - t < HOLD_WINDOW);
                    if holding {
                        player.set_scrub(dir);
                        scrub_deadline = now + SCRUB_TIMEOUT;
                        status(&player, if dir > 0 { "⏩ 3x" } else { "⏪ 3x" });
                    } else {
                        let secs = if key.modifiers.contains(KeyModifiers::SHIFT) {
                            30.0
                        } else {
                            15.0
                        };
                        player.skip(secs * dir as f32);
                        status(
                            &player,
                            &format!("{} {}s", if dir > 0 { "→" } else { "←" }, secs as i32),
                        );
                    }
                    last_arrow = Some((code, now));
                }
                code @ (KeyCode::Up | KeyCode::Down) => {
                    let delta = if code == KeyCode::Up { 0.25 } else { -0.25 };
                    let r = player.adjust_rate(delta);
                    status(&player, &format!("▶ {r}x"));
                }
                KeyCode::Char('q') | KeyCode::Esc => {
                    cancelled.store(true, Ordering::SeqCst);
                    sink.stop();
                    break;
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    cancelled.store(true, Ordering::SeqCst);
                    sink.stop();
                    break;
                }
                _ => {}
            }
        }
        let _ = crossterm::terminal::disable_raw_mode();
        eprintln!();
    });
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
pub fn lang_for(voice: &str) -> &'static str {
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

/// Serialize playback across vox processes: overlapping readouts queue
/// instead of talking over each other. The lock is released automatically on
/// exit; `vox --stop` clears the whole queue because it kills every vox
/// process (waiters included).
fn acquire_play_lock() -> Result<fs::File> {
    use fs2::FileExt;
    let dir = config::shared_dir();
    fs::create_dir_all(&dir)?;
    let f = fs::File::create(dir.join("play.lock"))?;
    if f.try_lock_exclusive().is_err() {
        eprintln!("queued: waiting for the current readout (vox --stop silences all)");
        config::log_event("info", "queued", "waiting for current readout");
        f.lock_exclusive()?;
    }
    Ok(f)
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

pub fn save_wav(path: &Path, samples: &[f32]) -> Result<()> {
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
    let result = run().await;
    if let Err(e) = &result {
        report_failure(e);
    }
    result
}

/// Failures from spawned readouts (tray, Claude hook) would otherwise vanish
/// into /dev/null: log them, and when stderr isn't a terminal raise a macOS
/// notification so the user learns why nothing was spoken.
fn report_failure(err: &anyhow::Error) {
    config::log_event("error", "vox", &format!("{err:#}"));
    notify_headless(&format!("{err:#}"));
}

/// Post a macOS notification, but only when nobody is watching stderr
/// (i.e. vox was spawned by the tray or the Claude hook).
fn notify_headless(msg: &str) {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() {
        return;
    }
    let msg: String = msg.replace('"', "'").chars().take(180).collect();
    let _ = Command::new("osascript")
        .args(["-e", &format!(r#"display notification "{msg}" with title "vox""#)])
        .status();
}

async fn run() -> Result<()> {
    let args = Args::parse();

    if args.list_voices {
        let mut entries: Vec<serde_json::Value> = VOICE_NAMES
            .iter()
            .map(|v| {
                serde_json::json!({
                    "path": v,
                    "label": providers::voice_label("kokoro", v),
                    "provider": "kokoro",
                    "provider_label": providers::provider_label("kokoro"),
                    "ready": true,
                })
            })
            .collect();
        let errors = config::provider_errors();
        for p in providers::cloud_providers() {
            let needs = match p.availability() {
                Availability::Ready => None,
                Availability::NeedsKey(var) => Some(var),
            };
            let error = errors
                .get(p.name())
                .and_then(|v| v["reason"].as_str())
                .map(String::from);
            for v in p.voices() {
                entries.push(serde_json::json!({
                    "path": format!("{}/{}", p.name(), v),
                    "label": providers::voice_label(p.name(), &v),
                    "provider": p.name(),
                    "provider_label": providers::provider_label(p.name()),
                    "ready": needs.is_none(),
                    "needs": needs,
                    "error": error,
                }));
            }
        }
        if args.json {
            println!("{}", serde_json::Value::Array(entries));
        } else {
            for e in entries {
                let suffix = match (e["needs"].as_str(), e["error"].as_str()) {
                    (Some(var), _) => format!("   (needs {var})"),
                    (None, Some(err)) => format!("   (error: {err})"),
                    _ => String::new(),
                };
                println!(
                    "{:<44} {}{}",
                    e["path"].as_str().unwrap_or(""),
                    e["label"].as_str().unwrap_or(""),
                    suffix
                );
            }
        }
        return Ok(());
    }
    if args.stop {
        return stop_others();
    }

    // espeak-ng looks for its data via this env var (then cwd / exe dir);
    // we keep espeak-ng-data in the vox cache dir, installed by --setup docs.
    if std::env::var_os("PIPER_ESPEAKNG_DATA_DIRECTORY").is_none()
        && cache_dir().join("espeak-ng-data").exists()
    {
        std::env::set_var("PIPER_ESPEAKNG_DATA_DIRECTORY", cache_dir());
    }

    if args.setup {
        ensure_models()?;
        eprintln!("Models ready in {}", cache_dir().display());
        return Ok(());
    }

    // Per-repo settings: flag/env wins, then the nearest .vox.json, then the
    // built-in default (CLI) or config.toml (TUI).
    let proj = config::project_overrides();

    if args.ui
        || (args.text.is_none() && args.file.is_none() && !args.clip && atty_stdin())
    {
        let (model_path, voices_path) = ensure_models()?;
        let tts = TTSKoko::new(
            model_path.to_str().context("bad model path")?,
            voices_path.to_str().context("bad voices path")?,
        )
        .await;
        let mut cfg = config::Config::load();
        if let Some(p) = &proj {
            if let Some(v) = p["voice"].as_str() {
                // any provider path; resolved and validated at synthesis
                cfg.voice = v.into();
            }
            if let Some(s) = p["speed"].as_f64() {
                cfg.speed = s as f32;
            }
            if let Some(b) = p["save_audio"].as_bool() {
                cfg.save_audio = b;
            }
            if let Some(d) = p["audio_dir"].as_str() {
                cfg.audio_dir = d.into();
            }
        }
        let mut provs: Vec<Box<dyn Provider>> =
            vec![Box::new(providers::kokoro::Kokoro { tts })];
        provs.extend(providers::cloud_providers());
        return tui::run(provs, cfg).await;
    }

    let voice = args
        .voice
        .clone()
        .or_else(|| {
            proj.as_ref()
                .and_then(|p| p["voice"].as_str().map(String::from))
        })
        .unwrap_or_else(|| "bm_george".into());
    let speed = args
        .speed
        .or_else(|| proj.as_ref().and_then(|p| p["speed"].as_f64().map(|s| s as f32)))
        .unwrap_or(1.0);

    let text = apply_lexicon(&read_text(&args)?, &load_lexicon());
    let vp = VoicePath::parse(&voice);

    let t0 = Instant::now();
    let provider: Box<dyn Provider> = if vp.provider == "kokoro" {
        if !VOICE_NAMES.contains(&vp.voice.as_str()) {
            bail!("unknown voice '{}' (try --list-voices)", vp.voice);
        }
        let (model_path, voices_path) = ensure_models()?;
        let tts = TTSKoko::new(
            model_path.to_str().context("bad model path")?,
            voices_path.to_str().context("bad voices path")?,
        )
        .await;
        eprintln!("Model loaded in {:.2}s", t0.elapsed().as_secs_f32());
        Box::new(providers::kokoro::Kokoro { tts })
    } else {
        let p = providers::cloud_providers()
            .into_iter()
            .find(|p| p.name() == vp.provider)
            .with_context(|| {
                format!("unknown provider '{}' (try --list-voices)", vp.provider)
            })?;
        if let Availability::NeedsKey(var) = p.availability() {
            bail!("provider '{}' needs the {var} environment variable", p.name());
        }
        p
    };
    let model = vp
        .model
        .clone()
        .unwrap_or_else(|| provider.default_model().to_string());
    let load = t0.elapsed();
    config::log_event(
        "info",
        "speak",
        &format!("{voice} · {} chars", text.chars().count()),
    );

    use std::sync::{atomic::AtomicBool, atomic::Ordering, Arc};

    let play = player::Player::new();

    // wait for any current readout to finish before making sound
    let _play_lock = if args.no_play {
        None
    } else {
        Some(acquire_play_lock()?)
    };

    let (_stream, sink) = if args.no_play {
        (None, None)
    } else {
        let (stream, handle) = OutputStream::try_default().context("no audio output device")?;
        let sink = Arc::new(Sink::try_new(&handle)?);
        sink.append(play.source());
        (Some(stream), Some(sink))
    };

    let cancelled = Arc::new(AtomicBool::new(false));
    if let Some(sink) = &sink {
        if atty_stdin() {
            eprintln!(
                "[space] pause · [←/→] 15s, shift 30s, hold = scrub 3x · [↑/↓] speed · [q/esc] cancel"
            );
            spawn_key_listener(sink.clone(), play.clone(), cancelled.clone());
        }
    }

    let mut first_audio: Option<f32> = None;
    let mut synth_err: Option<anyhow::Error> = None;
    let cancel_flag = cancelled.clone();
    let mut provider = provider;
    let mut active_voice = vp.voice.clone();
    let mut active_model = model;
    let mut fell_back = false;
    let sents = providers::sentences(&text);
    let mut i = 0;
    while i < sents.len() {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        let req = SynthReq {
            text: &sents[i],
            model: &active_model,
            voice: &active_voice,
            speed,
        };
        let result = provider.synth(&req, &mut |audio| {
            if first_audio.is_none() {
                let t = t0.elapsed().as_secs_f32();
                first_audio = Some(t);
                eprintln!(
                    "First audio at {t:.2}s (load {:.2}s + synth {:.2}s)",
                    load.as_secs_f32(),
                    t - load.as_secs_f32()
                );
            }
            play.append(audio);
            !cancel_flag.load(Ordering::SeqCst)
        });
        match result {
            Ok(_) => i += 1,
            // Cloud provider failed: remember why (menus gray it out), then
            // fall back to local kokoro so the readout still gets spoken.
            Err(e) if provider.name() != "kokoro" && !fell_back => {
                let name = provider.name();
                let reason = providers::short_reason(&format!("{e:#}"));
                config::record_provider_error(name, &reason);
                config::log_event(
                    "error",
                    "vox",
                    &format!("{name}: {e:#} — falling back to kokoro"),
                );
                eprintln!("{name} failed ({reason}) — falling back to kokoro");
                notify_headless(&format!("{name} failed ({reason}) — using Kokoro instead"));
                let (model_path, voices_path) = ensure_models()?;
                let tts = TTSKoko::new(
                    model_path.to_str().context("bad model path")?,
                    voices_path.to_str().context("bad voices path")?,
                )
                .await;
                provider = Box::new(providers::kokoro::Kokoro { tts });
                active_voice = "bm_george".into();
                active_model = "v1.0".into();
                fell_back = true;
                // retry the same sentence on kokoro
            }
            Err(e) => {
                synth_err = Some(e);
                break;
            }
        }
    }
    // a full run on a cloud provider clears its recorded error
    if !fell_back && synth_err.is_none() && provider.name() != "kokoro" && !sents.is_empty() {
        config::clear_provider_error(provider.name());
    }
    play.synth_done.store(true, Ordering::SeqCst);
    if let Some(e) = synth_err {
        if !cancelled.load(Ordering::SeqCst) {
            let _ = crossterm::terminal::disable_raw_mode();
            bail!("synthesis failed: {e}");
        }
    }

    let total = t0.elapsed().as_secs_f32();
    let synth = total - load.as_secs_f32();
    let audio_secs = play.len() as f32 / SAMPLE_RATE as f32;
    eprintln!(
        "Synthesized {audio_secs:.1}s of audio in {synth:.1}s ({:.1}x faster than realtime)",
        audio_secs / synth.max(0.001)
    );

    if !args.no_save {
        let out = out_path(&args, &text)?;
        save_wav(&out, &play.buf.read().unwrap())?;
        eprintln!("Saved {}", out.display());
    }

    if let Some(sink) = &sink {
        sink.sleep_until_end();
    }
    config::log_event(
        "info",
        "done",
        &format!(
            "{active_voice}{} · {audio_secs:.1}s audio",
            if fell_back { " (fallback)" } else { "" }
        ),
    );
    cancelled.store(true, Ordering::SeqCst); // ends the key listener
    let _ = crossterm::terminal::disable_raw_mode();
    Ok(())
}
