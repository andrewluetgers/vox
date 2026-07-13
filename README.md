# vox

Local streaming text-to-speech CLI in a single Rust binary. Kokoro-82M via
ONNX Runtime — no cloud, no API keys, no Python. First word in ~1–3 seconds,
synthesis 2.5–3.6× faster than realtime on an M1 Pro.

Default voice is `bm_george`, a mature British male.

## Install

```sh
git clone https://github.com/andrewluetgers/vox.git
cd vox
cargo build --release   # needs cmake (brew install cmake) for espeak-ng
cp target/release/vox ~/.local/bin/
mkdir -p ~/Library/Caches/vox
cp -r target/release/build/espeak-rs-sys-*/out/share/espeak-ng-data ~/Library/Caches/vox/
vox --setup   # one-time model download (~350 MB) to ~/Library/Caches/vox
```

The binary is ~34 MB with ONNX Runtime and espeak-ng statically linked.
Two data pieces live outside it: the model weights (downloaded once by
`--setup`) and espeak-ng's phoneme data (~9 MB, copied from the build as
above; vox finds it in its cache dir, or set
`PIPER_ESPEAKNG_DATA_DIRECTORY`).

Phonemization is espeak-ng — the same G2P Kokoro was trained with, so
pronunciation matches the reference Python implementation, including
proper British phonemes (`en-gb-x-rp`) for the `b*` voices.

## Usage

```sh
vox "hello there"            # speak a string
vox -f notes.txt             # read a file
pbpaste | vox                # read piped text
vox -c                       # read the clipboard
vox -v bf_emma -s 1.2 "…"    # other voice, faster pace
vox --no-play -o out.wav "…" # render to file silently
vox --no-save "…"            # just speak, keep nothing
vox --list-voices
```

Voices: `bm_george`, `bm_lewis`, `bm_daniel`, `bm_fable`, `bf_emma`,
`bf_isabella` (British); `am_adam`, `am_michael`, `af_heart`, `af_bella`,
`af_nicole`, `af_sarah` (American).

## Persistent reader UI

Run `vox` with no arguments (or `vox --ui`) to open a persistent
terminal UI, Claude-Code style: a scrolling transcript on top, a status
line, and an input box at the bottom. Type text, press Enter, and it
speaks — the submitted text appears dimmed and each word brightens to
white as it's spoken, so the transcript doubles as a progress indicator.

Pasted text collapses to a chip (`[pasted #1 · 843 chars]`) instead of
filling the input; Enter speaks pastes plus anything typed, Backspace on
an empty input removes the last chip.

- **Enter** — speak the input (new submissions queue up)
- **space** (input empty) — pause/resume · **←/→** — skip, hold to scrub
- **↑/↓** — playback speed (pitch-preserving time stretch — no chipmunk;
  scrubbing stays tape-style on purpose)
- **Esc** — stop: halts playback, cancels remaining synthesis, drops the queue
- **Ctrl-R** — repeat the last utterance · **Ctrl-P** — pick from the last 10
  spoken items (history is shared with the menu-bar app and Claude readouts)
- **PgUp/PgDn** — scroll history
- **Tab** — settings: voice, synthesis speed, audio folder, save on/off,
  cleanup-on-exit (delete this session's files when you quit)
- **Ctrl-C** — quit

Settings persist in `~/.config/vox/config.toml`. Audio files are written
to the configured folder (default `~/Music/vox`), one wav per utterance.

## Playback controls

While vox is speaking in an interactive terminal:

- **space** — pause / resume
- **← / →** — skip back / forward 15 s (**Shift** for 30 s)
- **hold ← / →** — scrub backward / forward at 3× (release to resume normal playback)
- **↑ / ↓** — playback speed ±0.25× (0.25–3×, tape-style: pitch follows rate)
- **q** or **Esc** or **Ctrl-C** — cancel

Skips and scrubs are capped at the edge of what's been synthesized — you
can't jump ahead of generation. Playback speed (↑/↓, applied live) is
independent of synthesis speed (`-s`, baked into the voice).

From anywhere else (another terminal, a script, a hotkey):

```sh
vox --stop    # silences any running vox
```

## Pronunciation fixes

The G2P is dictionary-based, so unusual words, names, and jargon can come
out oddly. Fix them with a lexicon at `~/.config/vox/lexicon.txt`
(override path with `VOX_LEXICON`) — one `word = respelling` per line,
matched whole-word and case-insensitively before synthesis:

```
# how it should sound, spelled phonetically
kokoro = koh-KOH-roh
qwen = kwen
nginx = engine-ex
```

## Claude Code integration

The [`claude/`](claude/) directory makes Claude Code talk to you: a Stop
hook speaks a Haiku-condensed summary of each Claude response through vox,
and a `/vox` skill gives conversational control — "vox, repeat that",
"read me the whole thing", "vox slower", "vox off". Install with:

```sh
./claude/install.sh
```

See [claude/README.md](claude/README.md) for how it works.

## Menu-bar app (spike)

[`tray/`](tray/) is a Tauri v2 menu-bar companion: tray controls for the
readouts (repeat last, a Recent menu of the last 10 spoken items, speed,
on/off), configurable global shortcuts (⌃⌥⌘V speaks the clipboard), a panel
with history and full settings (including per-repo overrides), and a JSON
socket at `~/.claude/vox/vox.sock`. It stays in feature parity with the
terminal UI above — same shared history and settings. See
[tray/README.md](tray/README.md).

## Defaults via environment

```sh
export VOX_VOICE=bm_george   # default voice
export VOX_SPEED=1.15        # default speech speed
```

## Per-project settings

A `.vox.json` at (or above) the current directory sets per-repo defaults —
the same file the [Claude Code integration](claude/) uses for per-repo
readout overrides:

```json
{ "voice": "bf_emma", "speed": 1.2 }
```

Precedence: CLI flag / env var → `.vox.json` → built-in default. The
persistent UI also picks up `voice`, `speed`, `save_audio`, and `audio_dir`
from it (settings saved from the UI still go to the global config.toml).

## Where things go

- Spoken audio is saved to `~/Music/vox/` (override with `VOX_AUDIO_DIR`,
  or pass `--no-save`).
- Models cache in `~/Library/Caches/vox` (override with `VOX_CACHE_DIR`).
  `VOX_MODEL_FILE` selects an alternate model file — e.g.
  `kokoro-v1.0.int8.onnx` for the 92 MB int8 model, which is smaller but
  ~2× slower than fp32 on Apple Silicon.

## Performance (M1 Pro, 32 GB)

| Stage | Time |
|---|---|
| Model load | ~0.4 s |
| First sound | ~1–3 s (first sentence synthesizes alone) |
| Synthesis pace | 2.5–3.6× faster than realtime, streams while playing |

## Development

```sh
cargo build --release            # vox binary
cargo test                       # unit tests + md2speech integration tests
(cd tray && cargo build)         # menu-bar app (Tauri v2)
rm -f ~/.local/bin/vox && cp target/release/vox ~/.local/bin/  # install
./claude/install.sh              # (re)sync the Claude Code integration
```

Gotcha: on Apple Silicon, `cp` **over** an existing binary invalidates its
code signature and macOS kills it on launch (`zsh: killed`, exit 137) —
always remove the old file first, as above.

Everything this repo does on a machine is reproducible from the repo:
`claude/install.sh` materializes the hook/skill/filter into `~/.claude` and
registers the Stop hook; the tray app and vox are plain cargo builds; files
under `~/.claude/vox/` (state.json, history.jsonl, projects.json) are
runtime state that auto-creates with defaults. See CLAUDE.md for repo
conventions.

## History

v0.1 was a Python implementation on mlx-audio (see git history). The Rust
port cut first-sound latency from ~8 s to ~1–3 s and replaced a Python
environment with one binary.
