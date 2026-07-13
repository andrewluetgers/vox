//! Persistent reader UI: scrolling karaoke transcript, status line, input box.
//!
//! Text you submit is queued for synthesis; the transcript shows it dimmed
//! and brightens each word as the playback cursor passes it. Synthesis runs
//! on a worker thread appending into the shared Player buffer; per-word
//! timing is estimated by character weight within each synthesized sentence.

use crate::config::Config;
use crate::player::{Player, SAMPLE_RATE};
use crate::{lang_for, save_wav, VOICE_NAMES};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use kokoros::tts::koko::TTSKoko;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use rodio::{OutputStream, Sink};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

struct Word {
    text: String,
    /// sample index at which this word has been fully spoken
    end: u64,
}

struct Utterance {
    words: Vec<Word>,
    start: u64,
    end: u64,
    done: bool,
}

#[derive(Default)]
struct Shared {
    utterances: Vec<Utterance>,
    synthesizing: bool,
    saved_files: Vec<std::path::PathBuf>,
}

struct Job {
    text: String,
    voice: String,
    speed: f32,
    save_dir: Option<std::path::PathBuf>,
}

/// Split text into sentences (bounded length) for incremental synthesis.
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        cur.push(ch);
        if matches!(ch, '.' | '!' | '?' | ';' | ':' | '\n') || cur.len() > 300 {
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
            }
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn synth_worker(
    tts: TTSKoko,
    rx: mpsc::Receiver<Job>,
    shared: Arc<Mutex<Shared>>,
    player: Player,
    cancel: Arc<AtomicBool>,
) {
    while let Ok(job) = rx.recv() {
        cancel.store(false, Ordering::SeqCst);
        let start = player.len() as u64;
        {
            let mut sh = shared.lock().unwrap();
            sh.synthesizing = true;
            sh.utterances.push(Utterance {
                words: Vec::new(),
                start,
                end: start,
                done: false,
            });
        }
        let mut was_cancelled = false;
        for sentence in sentences(&job.text) {
            if cancel.load(Ordering::SeqCst) {
                was_cancelled = true;
                break;
            }
            let audio = match tts.tts_raw_audio(
                &sentence,
                lang_for(&job.voice),
                &job.voice,
                job.speed,
                None,
                None,
                None,
                None,
            ) {
                Ok(a) => a,
                Err(_) => continue,
            };
            let sent_start = player.len() as u64;
            player.append(&audio);
            let sent_len = audio.len() as u64;

            // estimate word boundaries by character weight within the sentence
            let words: Vec<&str> = sentence.split_whitespace().collect();
            let total_chars: usize = words.iter().map(|w| w.len() + 1).sum();
            let mut acc = 0usize;
            let mut sh = shared.lock().unwrap();
            let utt = sh.utterances.last_mut().unwrap();
            for w in &words {
                acc += w.len() + 1;
                let end = sent_start + sent_len * acc as u64 / total_chars.max(1) as u64;
                utt.words.push(Word {
                    text: w.to_string(),
                    end,
                });
            }
            utt.end = sent_start + sent_len;
        }
        if was_cancelled || cancel.load(Ordering::SeqCst) {
            // Esc means stop everything: drop any queued submissions too
            while rx.try_recv().is_ok() {}
        }
        let mut sh = shared.lock().unwrap();
        sh.synthesizing = false;
        if let Some(utt) = sh.utterances.last_mut() {
            utt.done = true;
            let (start, end) = (utt.start as usize, utt.end as usize);
            if let Some(dir) = &job.save_dir {
                if end > start {
                    let _ = std::fs::create_dir_all(dir);
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    let slug: String = job
                        .text
                        .to_lowercase()
                        .chars()
                        .take(32)
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                        .collect();
                    let path = dir.join(format!("{stamp}-{}.wav", slug.trim_matches('-')));
                    let buf = player.buf.read().unwrap();
                    if save_wav(&path, &buf[start..end.min(buf.len())]).is_ok() {
                        sh.saved_files.push(path);
                    }
                }
            }
        }
    }
}

pub async fn run(tts: TTSKoko, mut cfg: Config) -> Result<()> {
    let player = Player::new();
    let (_stream, handle) = OutputStream::try_default()?;
    let sink = Sink::try_new(&handle).map_err(|e| anyhow::anyhow!("audio: {e}"))?;
    sink.append(player.source());

    let shared = Arc::new(Mutex::new(Shared::default()));
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Job>();
    {
        let (tts, shared, player, cancel) =
            (tts.clone(), shared.clone(), player.clone(), cancel.clone());
        std::thread::spawn(move || synth_worker(tts, rx, shared, player, cancel));
    }

    let mut terminal = ratatui::init();
    // deliver pastes as a single Event::Paste instead of a flood of key events
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
    let res = ui_loop(
        &mut terminal,
        &sink,
        &player,
        &shared,
        &cancel,
        &tx,
        &mut cfg,
    );
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
    ratatui::restore();
    sink.stop();

    let _ = cfg.save();
    if cfg.cleanup_on_exit {
        for f in &shared.lock().unwrap().saved_files {
            let _ = std::fs::remove_file(f);
        }
        eprintln!("Cleaned up this session's audio files.");
    } else {
        let n = shared.lock().unwrap().saved_files.len();
        if n > 0 {
            eprintln!("{n} audio file(s) in {}", cfg.audio_dir);
        }
    }
    res
}

struct UiState {
    input: String,
    /// pasted blocks, shown as collapsed chips instead of filling the input
    pastes: Vec<String>,
    scroll_up: u16,
    settings_open: bool,
    settings_sel: usize,
    editing: Option<String>,
    tick: usize,
    /// recent-history picker (Ctrl-P): last 10 from the shared history,
    /// ←/→ cycles a source filter (all, claude, tui, …)
    hist_open: bool,
    hist_sel: usize,
    hist_items: Vec<(String, String)>,
    hist_all: Vec<(String, String)>,
    hist_filters: Vec<String>,
    hist_filter: usize,
    /// last text spoken this session (falls back to shared last-spoken.txt)
    last_text: Option<String>,
}

fn ui_loop(
    terminal: &mut ratatui::DefaultTerminal,
    sink: &Sink,
    player: &Player,
    shared: &Arc<Mutex<Shared>>,
    cancel: &Arc<AtomicBool>,
    tx: &mpsc::Sender<Job>,
    cfg: &mut Config,
) -> Result<()> {
    let mut st = UiState {
        input: String::new(),
        pastes: Vec::new(),
        scroll_up: 0,
        settings_open: false,
        settings_sel: 0,
        editing: None,
        tick: 0,
        hist_open: false,
        hist_sel: 0,
        hist_items: Vec::new(),
        hist_all: Vec::new(),
        hist_filters: Vec::new(),
        hist_filter: 0,
        last_text: crate::config::last_spoken(),
    };
    // hold-to-scrub detection (same scheme as one-shot mode)
    let mut last_arrow: Option<(KeyCode, Instant)> = None;
    let mut scrub_deadline = Instant::now();
    // Esc pressed: keep pinning the cursor to the end until the synthesis
    // worker has actually stopped appending, otherwise freshly generated
    // audio lands beyond the cursor and playback resumes into it.
    let mut stopping = false;

    loop {
        st.tick = st.tick.wrapping_add(1);
        if player.scrubbing() != 0 && Instant::now() > scrub_deadline {
            player.set_scrub(0);
        }
        if stopping {
            player.jump_to_end();
            if !shared.lock().unwrap().synthesizing {
                stopping = false;
            }
        }
        terminal.draw(|f| draw(f, &st, sink, player, shared, cfg))?;

        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        let key = match event::read()? {
            Event::Key(key) => key,
            Event::Paste(s) => {
                if let Some(buf) = &mut st.editing {
                    buf.push_str(s.trim());
                } else if !s.trim().is_empty() {
                    st.pastes.push(s);
                }
                continue;
            }
            _ => continue,
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }

        // global
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(());
        }

        if st.settings_open {
            handle_settings_key(&mut st, key.code, cfg);
            continue;
        }

        // recent-history picker (parity with the tray app's Recent menu)
        if st.hist_open {
            match key.code {
                KeyCode::Esc | KeyCode::Tab => st.hist_open = false,
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    st.hist_open = false
                }
                KeyCode::Up => st.hist_sel = st.hist_sel.saturating_sub(1),
                KeyCode::Down => {
                    st.hist_sel = (st.hist_sel + 1).min(st.hist_items.len().saturating_sub(1))
                }
                code @ (KeyCode::Left | KeyCode::Right) => {
                    let n = st.hist_filters.len();
                    if n > 0 {
                        st.hist_filter = if code == KeyCode::Right {
                            (st.hist_filter + 1) % n
                        } else {
                            (st.hist_filter + n - 1) % n
                        };
                        apply_hist_filter(&mut st);
                    }
                }
                KeyCode::Enter => {
                    if let Some((_, text)) = st.hist_items.get(st.hist_sel) {
                        st.last_text = Some(text.clone());
                        tx.send(Job {
                            text: text.clone(),
                            voice: cfg.voice.clone(),
                            speed: cfg.speed,
                            save_dir: cfg.save_audio.then(|| cfg.audio_dir_path()),
                        })
                        .ok();
                        st.scroll_up = 0;
                    }
                    st.hist_open = false;
                }
                _ => {}
            }
            continue;
        }

        match key.code {
            KeyCode::Tab => st.settings_open = true,
            KeyCode::Enter => {
                let mut parts: Vec<String> = st.pastes.drain(..).collect();
                let typed = st.input.trim().to_string();
                if !typed.is_empty() {
                    parts.push(typed);
                }
                let text = parts.join("\n\n").trim().to_string();
                if !text.is_empty() {
                    st.last_text = Some(text.clone());
                    crate::config::log_history("tui", &text);
                    tx.send(Job {
                        text,
                        voice: cfg.voice.clone(),
                        speed: cfg.speed,
                        save_dir: cfg.save_audio.then(|| cfg.audio_dir_path()),
                    })
                    .ok();
                    st.input.clear();
                    st.scroll_up = 0;
                }
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // repeat last spoken (this session, else the shared last-spoken)
                if let Some(text) = st.last_text.clone().or_else(crate::config::last_spoken) {
                    tx.send(Job {
                        text,
                        voice: cfg.voice.clone(),
                        speed: cfg.speed,
                        save_dir: cfg.save_audio.then(|| cfg.audio_dir_path()),
                    })
                    .ok();
                    st.scroll_up = 0;
                }
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                st.hist_all = crate::config::recent_history(200);
                let mut filters = vec!["all".to_string()];
                for (source, _) in &st.hist_all {
                    if !filters.contains(source) {
                        filters.push(source.clone());
                    }
                }
                st.hist_filters = filters;
                st.hist_filter = 0;
                apply_hist_filter(&mut st);
                st.hist_open = !st.hist_all.is_empty();
            }
            KeyCode::Backspace => {
                // backspace on an empty input removes the last paste chip
                if st.input.pop().is_none() {
                    st.pastes.pop();
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                st.input.clear();
            }
            KeyCode::Esc => {
                // stop current speech and clear anything queued
                cancel.store(true, Ordering::SeqCst);
                player.jump_to_end();
                stopping = true;
            }
            KeyCode::Char(' ') if st.input.is_empty() => {
                if sink.is_paused() {
                    sink.play();
                } else {
                    sink.pause();
                }
            }
            code @ (KeyCode::Left | KeyCode::Right) => {
                let dir: i8 = if code == KeyCode::Left { -1 } else { 1 };
                let now = Instant::now();
                let holding = key.kind == KeyEventKind::Repeat
                    || matches!(last_arrow, Some((c, t)) if c == code && now - t < Duration::from_millis(250));
                if holding {
                    player.set_scrub(dir);
                    scrub_deadline = now + Duration::from_millis(300);
                } else {
                    let secs = if key.modifiers.contains(KeyModifiers::SHIFT) {
                        30.0
                    } else {
                        15.0
                    };
                    player.skip(secs * dir as f32);
                }
                last_arrow = Some((code, now));
            }
            KeyCode::Up => {
                player.adjust_rate(0.25);
            }
            KeyCode::Down => {
                player.adjust_rate(-0.25);
            }
            KeyCode::PageUp => st.scroll_up = st.scroll_up.saturating_add(10),
            KeyCode::PageDown => st.scroll_up = st.scroll_up.saturating_sub(10),
            KeyCode::Char(ch) => st.input.push(ch),
            _ => {}
        }
    }
}

const SETTINGS: &[&str] = &[
    "voice",
    "synthesis speed",
    "audio folder",
    "save audio",
    "cleanup on exit",
];

fn handle_settings_key(st: &mut UiState, code: KeyCode, cfg: &mut Config) {
    if let Some(buf) = &mut st.editing {
        match code {
            KeyCode::Enter => {
                cfg.audio_dir = st.editing.take().unwrap();
                let _ = cfg.save();
            }
            KeyCode::Esc => st.editing = None,
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Char(ch) => buf.push(ch),
            _ => {}
        }
        return;
    }
    match code {
        KeyCode::Tab | KeyCode::Esc => {
            st.settings_open = false;
            let _ = cfg.save();
        }
        KeyCode::Up => st.settings_sel = st.settings_sel.saturating_sub(1),
        KeyCode::Down => st.settings_sel = (st.settings_sel + 1).min(SETTINGS.len() - 1),
        KeyCode::Left | KeyCode::Right | KeyCode::Enter => {
            let fwd = code != KeyCode::Left;
            match st.settings_sel {
                0 => {
                    let i = VOICE_NAMES
                        .iter()
                        .position(|v| *v == cfg.voice)
                        .unwrap_or(0);
                    let n = VOICE_NAMES.len();
                    cfg.voice =
                        VOICE_NAMES[if fwd { (i + 1) % n } else { (i + n - 1) % n }].to_string();
                }
                1 => {
                    cfg.speed = (cfg.speed + if fwd { 0.1 } else { -0.1 }).clamp(0.5, 2.0);
                    cfg.speed = (cfg.speed * 10.0).round() / 10.0;
                }
                2 => {
                    if code == KeyCode::Enter {
                        st.editing = Some(cfg.audio_dir.clone());
                    }
                }
                3 => cfg.save_audio = !cfg.save_audio,
                4 => cfg.cleanup_on_exit = !cfg.cleanup_on_exit,
                _ => {}
            }
        }
        _ => {}
    }
}

/// Recompute the visible picker items for the current source filter.
fn apply_hist_filter(st: &mut UiState) {
    let filter = st.hist_filters.get(st.hist_filter).cloned().unwrap_or_default();
    st.hist_items = st
        .hist_all
        .iter()
        .filter(|(source, _)| filter == "all" || *source == filter)
        .take(10)
        .cloned()
        .collect();
    st.hist_sel = 0;
}

fn fmt_time(samples: f64) -> String {
    let s = samples / SAMPLE_RATE as f64;
    format!("{}:{:02}", (s as u64) / 60, (s as u64) % 60)
}

fn draw(
    f: &mut ratatui::Frame,
    st: &UiState,
    sink: &Sink,
    player: &Player,
    shared: &Arc<Mutex<Shared>>,
    cfg: &Config,
) {
    let [text_area, status_area, input_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(3),
    ])
    .areas(f.area());

    let pos = player.pos() as u64;
    let sh = shared.lock().unwrap();

    // ---- transcript with karaoke highlighting ----
    let mut lines: Vec<Line> = Vec::new();
    for utt in &sh.utterances {
        let mut spans: Vec<Span> = Vec::new();
        let mut prev_end = utt.start;
        for w in &utt.words {
            let style = if pos >= w.end {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if pos >= prev_end {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Span::styled(format!("{} ", w.text), style));
            prev_end = w.end;
        }
        lines.push(Line::from(spans));
        lines.push(Line::default());
    }
    let para = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    let total = para.line_count(text_area.width) as u16;
    let max_scroll = total.saturating_sub(text_area.height);
    let scroll = max_scroll.saturating_sub(st.scroll_up.min(max_scroll));
    f.render_widget(para.scroll((scroll, 0)), text_area);

    // ---- status line ----
    let len = player.len() as f64;
    let state = if sh.synthesizing {
        format!("{} synthesizing", SPINNER[st.tick % SPINNER.len()])
    } else if sink.is_paused() {
        "⏸ paused".into()
    } else if (player.pos()) < len - 1.0 {
        "▶ speaking".into()
    } else {
        "● idle".into()
    };
    let status = Line::from(vec![
        Span::styled(
            format!(" {state} "),
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(format!(
            " {} / {}  {:.2}x  {}  ",
            fmt_time(player.pos()),
            fmt_time(len),
            player.rate(),
            cfg.voice
        )),
        Span::styled(
            "tab settings · ^R repeat · ^P recent · esc stop · space pause · ←/→ skip · ↑/↓ speed · ^C quit",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(status), status_area);

    // ---- input box ----
    let mut spans: Vec<Span> = Vec::new();
    for (i, p) in st.pastes.iter().enumerate() {
        spans.push(Span::styled(
            format!("[pasted #{} · {} chars] ", i + 1, p.chars().count()),
            Style::default().fg(Color::Cyan),
        ));
    }
    spans.push(Span::raw(format!("{}▌", st.input)));
    let input = Paragraph::new(Line::from(spans))
        .block(Block::default().borders(Borders::ALL).title(" vox "));
    f.render_widget(input, input_area);

    // ---- recent-history popup (Ctrl-P) ----
    if st.hist_open {
        let w = f.area().width.saturating_sub(8).min(72).max(30);
        let h = (st.hist_items.len() as u16 + 4).min(f.area().height.saturating_sub(2));
        let area = Rect::new(
            (f.area().width - w) / 2,
            (f.area().height.saturating_sub(h)) / 3,
            w,
            h,
        );
        f.render_widget(Clear, area);
        let mut lines: Vec<Line> = Vec::new();
        let text_w = w.saturating_sub(14) as usize;
        for (i, (source, text)) in st.hist_items.iter().enumerate() {
            let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let mut shown: String = one_line.chars().take(text_w).collect();
            if one_line.chars().count() > text_w {
                shown.push('…');
            }
            let style = if i == st.hist_sel {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default()
            };
            lines.push(Line::styled(format!(" {source:<8} {shown}"), style));
        }
        lines.push(Line::default());
        lines.push(Line::styled(
            " ↑/↓ select · ←/→ source filter · enter speak · esc close",
            Style::default().fg(Color::DarkGray),
        ));
        let filter = st.hist_filters.get(st.hist_filter).cloned().unwrap_or_default();
        let title = if filter == "all" || filter.is_empty() {
            " recent ".to_string()
        } else {
            format!(" recent · {filter} ")
        };
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .block(Block::default().borders(Borders::ALL).title(title)),
            area,
        );
    }

    // ---- settings popup ----
    if st.settings_open {
        let w = 52.min(f.area().width.saturating_sub(4));
        let h = (SETTINGS.len() as u16 + 4).min(f.area().height.saturating_sub(2));
        let area = Rect::new(
            (f.area().width - w) / 2,
            (f.area().height.saturating_sub(h)) / 3,
            w,
            h,
        );
        f.render_widget(Clear, area);
        let values = [
            cfg.voice.clone(),
            format!("{:.1}x", cfg.speed),
            match &st.editing {
                Some(buf) => format!("{buf}▌"),
                None => cfg.audio_dir.clone(),
            },
            if cfg.save_audio { "on" } else { "off" }.into(),
            if cfg.cleanup_on_exit { "on" } else { "off" }.into(),
        ];
        let mut lines: Vec<Line> = Vec::new();
        for (i, (name, value)) in SETTINGS.iter().zip(values.iter()).enumerate() {
            let sel = i == st.settings_sel;
            let style = if sel {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default()
            };
            lines.push(Line::styled(format!(" {name:<18} {value}"), style));
        }
        lines.push(Line::default());
        lines.push(Line::styled(
            " ←/→ change · enter edit/toggle · tab close",
            Style::default().fg(Color::DarkGray),
        ));
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .block(Block::default().borders(Borders::ALL).title(" settings ")),
            area,
        );
    }
}
