# vox-tray (spike)

A macOS menu-bar companion for vox, built with Tauri v2. It lives in the
menu bar (no dock icon), drives the same `~/.claude/vox/state.json` the
[Claude Code integration](../claude/) reads, and exposes a local socket so
other processes can speak through one place.

## What it does

- **Tray menu**: toggle "Speak Claude replies" (flips `enabled` in
  state.json, so it mutes the Claude Stop hook), stop speaking, speak
  clipboard, speed presets, a small panel window with a speak box, quit.
- **Global shortcut ⌃⌥⌘V**: speak the clipboard (vox `-c`); press it again
  while speaking to stop. Workflow: ⌘C, then ⌃⌥⌘V.
- **Socket API** at `~/.claude/vox/vox.sock` — newline-delimited JSON,
  one reply line per command:

  ```sh
  echo '{"cmd":"speak","text":"hello from the socket"}' | nc -U ~/.claude/vox/vox.sock
  echo '{"cmd":"stop"}'      | nc -U ~/.claude/vox/vox.sock
  echo '{"cmd":"clipboard"}' | nc -U ~/.claude/vox/vox.sock
  echo '{"cmd":"status"}'    | nc -U ~/.claude/vox/vox.sock
  echo '{"cmd":"set","speed":1.4,"enabled":true}' | nc -U ~/.claude/vox/vox.sock
  ```

## Run

Needs the `vox` binary on PATH (or at `~/.local/bin/vox`) — the app shells
out to it for synthesis in this spike.

```sh
cd tray
cargo build            # first build pulls the Tauri stack, takes a while
./target/debug/vox-tray &
```

Quit from the tray menu. Note: macOS may ask for Accessibility/Input
Monitoring permission the first time the global shortcut is registered.

## Spike notes / what's next

- Synthesis is a subprocess per utterance; the real version should link
  vox's synthesis+player code as a library crate so the daemon owns the
  audio engine and can pause/scrub/re-speed live.
- The Claude Stop hook could prefer the socket when `vox.sock` exists
  (falling back to spawning `vox`), which would serialize all speech
  through the daemon.
- No bundling/codesigning yet (`bundle.active: false`); `cargo tauri build`
  + Developer ID notarization is the distribution path, no App Store needed.
