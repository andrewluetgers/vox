# Claude Code voice readouts

Make Claude Code talk to you through vox. When Claude finishes a response, a
Stop hook condenses it into a few spoken sentences with a headless Haiku
agent and reads it aloud — hands-free status updates instead of a wall of
text. A `/vox` skill gives you conversational control over the readout.

## Install

Build and install the vox binary first (main [README](../README.md)), then:

```sh
./claude/install.sh
```

The installer copies the hook script to `~/.claude/vox/`, the skill to
`~/.claude/skills/vox/`, and merges an async Stop hook into
`~/.claude/settings.json` — it's idempotent and preserves everything already
in your settings. In an already-running Claude Code session, open `/hooks`
once (or restart) so the new hook loads. Requires `jq`; uses the `claude`
CLI at runtime for summarization.

For a fresh machine or cloud session, the whole thing is:

```sh
git clone https://github.com/andrewluetgers/vox.git
cd vox
# build vox per the main README, then:
./claude/install.sh
```

Uninstall with `./claude/install.sh --uninstall`.

## How it works

- **`vox-speak.sh`** (the Stop hook, installed to `~/.claude/vox/`): pulls
  Claude's final message from the session transcript, saves the original to
  `~/.claude/vox/last-full.txt`, then speaks it. Responses over
  `verbatim_max` chars (default 300) are first rewritten into 1–3 spoken
  sentences by a headless Haiku call (`claude -p --no-session-persistence`,
  tools disabled, the summarizer prompt as its system prompt) — a separate
  process that never touches your chat thread or writes a transcript. The
  hook runs async, so your prompt is never blocked; it skips re-speaking on
  resume/clear/compact, logs every readout to `history.jsonl`, and caps
  runaway summaries at ~900 chars.
- **`skills/vox/SKILL.md`** (the `/vox` skill): teaches Claude that "vox,
  ..." is about the audio, not the code. Supports: `stop`, `off`/`on`,
  `repeat that`, `read me the whole thing` (the original, unsummarized
  output), `slower`/`faster`, `voice <name>`, `status`, changing or
  resetting the summarizer prompt, and audio saving.
- **`~/.claude/vox/state.json`**: settings shared by the hook, the skill,
  and the [vox-tray app](../tray/). All keys optional; defaults in
  parentheses: `enabled` (true), `voice` (bm_george), `speed` (1.1),
  `verbatim_max` (300), `summary_prompt` (built-in), `save_audio` (false —
  readouts aren't kept), `audio_dir` (~/Music/vox), `audio_ttl_minutes`
  (20; the tray app prunes older wavs when saving is on).
- **Per-project overrides**: a `.vox.json` at the project root overrides any
  state.json key for that repo — e.g. `{"voice": "bf_emma"}` to give a
  project its own voice, `{"summary_prompt": "..."}` for a custom readout
  style, or `{"enabled": false}` to silence a noisy project. The hook finds
  it by walking up from its working directory.

One subtlety: after a vox command like "repeat that", Claude's own
confirmation would trigger a fresh readout that talks over the replay. The
skill drops a `~/.claude/vox/skip-next` flag file that the hook consumes to
stay silent exactly once.

Note: if `ANTHROPIC_API_KEY` is set in your environment, the hook unsets it
for the summarizer call so `claude -p` uses your claude.ai subscription
login instead of API credits.
