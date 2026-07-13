# vox-tray (spike)

A macOS menu-bar companion for vox, built with Tauri v2. It lives in the
menu bar (no dock icon), drives the same `~/.claude/vox/state.json` the
[Claude Code integration](../claude/) reads, and exposes a local socket so
other processes can speak through one place.

## What it does

- **Tray menu**: toggle "Speak Claude replies" (flips `enabled` in
  state.json, so it mutes the Claude Stop hook), stop speaking, speak
  clipboard, speed presets, panel window, quit.
- **Panel window** (Open vox… in the tray): three tabs —
  - **Speak**: text box (⌘↩ speaks, Esc stops), speak-clipboard button.
  - **History**: the last 100 readouts from every source (Claude hook,
    clipboard, socket, panel) with replay buttons.
  - **Settings**: readouts on/off; voice picker that speaks a sample of the
    voice you choose; speed with a test button; verbatim threshold; the
    summarizer prompt with save/reset-to-default; a shortcut recorder for
    every action (click Record, press keys — conflicts and invalid combos
    are reported inline); audio saving (default off) with folder picker,
    open-folder button, and delete-after TTL.
- **Global shortcuts** (all configurable, defaults):
  - ⌃⌥⌘V — speak clipboard; press again while speaking to stop
  - ⌃⌥⌘S — stop speaking
  - ⌃⌥⌘R — replay the last readout
  - toggle readouts — unbound by default
- **Socket API** at `~/.claude/vox/vox.sock` — newline-delimited JSON,
  one reply line per command:

  ```sh
  echo '{"cmd":"speak","text":"hello from the socket"}' | nc -U ~/.claude/vox/vox.sock
  echo '{"cmd":"stop"}'      | nc -U ~/.claude/vox/vox.sock
  echo '{"cmd":"clipboard"}' | nc -U ~/.claude/vox/vox.sock
  echo '{"cmd":"status"}'    | nc -U ~/.claude/vox/vox.sock
  echo '{"cmd":"set","speed":1.4,"summary_prompt":"…"}' | nc -U ~/.claude/vox/vox.sock
  ```

## Settings model

Defaults live in code (and mirrored in the Claude hook); `state.json` stores
only overrides, so resetting a setting removes its key. `set`/Settings
changes apply to the next utterance and to the Claude hook's readouts.
Per-project overrides live in a `.vox.json` at a repo root (see
[claude/README.md](../claude/README.md)); those affect the hook, which runs
inside the project, not this app.

Two retention systems, both explicit in Settings:

- **Text history** (on by default): spoken text logs to `history.jsonl` for
  the History tab; entries older than `history_ttl_minutes` (default 20,
  0 = keep) are pruned on write and every two minutes while the app runs.
  The most recent readout is always kept separately so replay-last works
  with history off.
- **Audio files** (off by default): uncompressed WAV, ~3 MB per spoken
  minute — the Settings section carries a storage warning. When on, wavs
  older than `audio_ttl_minutes` are pruned on the same timer.

Markdown handling is a deterministic filter (`~/.claude/vox/md2speech.sh`),
not a prompt — "Edit rules" in Settings opens it in your editor.

## Run

Needs the `vox` binary on PATH (or at `~/.local/bin/vox`) — the app shells
out to it for synthesis in this spike.

```sh
cd tray
cargo build            # first build pulls the Tauri stack, takes a while
./target/debug/vox-tray &
```

Quit from the tray menu.

## Spike notes / what's next

- Synthesis is a subprocess per utterance; the real version should link
  vox's synthesis+player code as a library crate so the daemon owns the
  audio engine and can pause/scrub/re-speed live.
- The Claude Stop hook could prefer the socket when `vox.sock` exists
  (falling back to spawning `vox`), which would serialize all speech
  through the daemon.
- The tray "enabled" checkbox doesn't refresh if state.json is edited
  externally while the menu is open.
- The vox CLI and TUI honor `.vox.json` (voice/speed, plus save_audio and
  audio_dir in the TUI) with flag/env taking precedence; the TUI has no
  visual indicator yet that an override is active, and its settings screen
  still saves to the global config.toml.
- No bundling/codesigning yet (`bundle.active: false`); `cargo tauri build`
  + Developer ID notarization is the distribution path, no App Store needed.
