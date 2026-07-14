# vox — notes for Claude

Local streaming TTS. One repo, four surfaces that share state:

- `src/` — the vox binary: one-shot CLI + persistent terminal UI (`vox` with
  no args). Rust, ratatui.
- `tray/` — vox-tray, the macOS menu-bar app. Rust, Tauri v2; webview panel
  in `tray/ui/index.html`.
- `claude/` — Claude Code integration: Stop hook (spoken summaries of Claude
  responses), /vox skill, md2speech filter, installer.
- Shared state: `~/.claude/vox/` — state.json (settings), history.jsonl,
  last-spoken.txt, vox.sock, projects.json. Per-repo overrides: `.vox.json`
  at a repo root.

## UI parity rule

**The menu-bar app (tray/) and the persistent terminal UI (src/tui.rs) are
siblings and must stay roughly functionally equivalent.** When adding a
feature to one, add the counterpart to the other in the same change:
same shared state, same semantics, idiomatic presentation (menu items and
panels in the tray; keybindings and popups in the TUI). If a feature
genuinely wouldn't make sense in the terminal UI (or vice versa), don't
force it — flag the asymmetry to the user and discuss instead.

Current equivalences: repeat-last (tray menu "Repeat last" ↔ TUI Ctrl-R),
recent history (tray "Recent" submenu ↔ TUI Ctrl-P picker, both last 10
from shared history.jsonl), history source filter (panel History dropdowns
↔ TUI picker ←/→ cycle), voice quick-pick with provider sections (tray
"Voice" submenu grouped by provider ↔ TUI Tab-settings voice field cycling
all providers' voices; both driven by the same provider registry / `vox
--list-voices`), settings (tray Settings tab ↔ TUI Tab popup),
stop/pause/speed transport in both.

Known accepted asymmetry: per-repo history filtering is panel-only (a
second filter axis in the small TUI popup was judged clutter — revisit if
asked).

## Conventions

- Settings live in `~/.claude/vox/state.json` as overrides over in-code
  defaults; "reset" = delete the key. The hook (`claude/vox-speak.sh`) and
  tray (`tray/src/main.rs` DEFAULT_PROMPT) duplicate the summarizer default
  prompt — keep them identical.
- After editing anything in `claude/`, run `./claude/install.sh` to sync the
  installed copies in `~/.claude/`.
- Markdown-to-speech is a deterministic filter (`claude/md2speech.sh`), not
  an LLM prompt. Never let TTS speak raw markdown syntax; structure becomes
  pauses.
- Text spoken by any surface should be logged to the shared history
  (respecting `save_history`) and update last-spoken.txt.
- `vox --stop` kills all vox processes — it's the universal silencer.
- Readouts queue rather than interrupt: one-shot playback serializes on an
  exclusive lock of `~/.claude/vox/play.lock`. Neither the hook nor the tray
  calls `--stop` before speaking; `--stop` silences current *and* queued.
  (Accepted asymmetry: the TUI plays outside this queue.)
- Voices are path strings: bare (`bm_george`) = local kokoro;
  `provider/voice` or `provider/model/voice` = cloud (openai, elevenlabs,
  xai, groq — see `docs/providers.md` and `src/providers/`). API keys come
  from standard env vars only (`OPENAI_API_KEY`, …), never stored in any
  config file. `vox --list-voices` shows everything with key status.
- Installing a locally built binary: `rm` the old one first, then `cp`
  (`cp` over an existing signed binary invalidates the signature and macOS
  SIGKILLs it — "zsh: killed", exit 137).
- Run `cargo test` before committing: unit tests live next to the code
  (config history/overrides, TUI history filter) and `tests/md2speech.rs`
  exercises the actual shell filter. Add tests alongside new pure logic.
- Nothing about the working setup may live only in a Claude session or in
  `~/.claude` by hand: if it's needed to reproduce the setup, it belongs in
  the repo (usually `claude/install.sh` or documented in a README).
