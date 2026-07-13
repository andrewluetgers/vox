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
  sentences by `claude -p --model haiku --no-session-persistence` — a
  separate headless process that never touches your chat thread or writes a
  transcript. The hook runs async, so your prompt is never blocked, and it
  skips re-speaking on resume/clear/compact.
- **`skills/vox/SKILL.md`** (the `/vox` skill): teaches Claude that "vox,
  ..." is about the audio, not the code. Supports: `stop`, `off`/`on`,
  `repeat that`, `read me the whole thing` (the original, unsummarized
  output), `slower`/`faster`, `voice <name>`, `status`.
- **`~/.claude/vox/state.json`**: `enabled`, `voice`, `speed`,
  `verbatim_max` — read live by the hook on every readout, edited by the
  skill or by hand.

One subtlety: after a vox command like "repeat that", Claude's own
confirmation would trigger a fresh readout that talks over the replay. The
skill drops a `~/.claude/vox/skip-next` flag file that the hook consumes to
stay silent exactly once.

Note: if `ANTHROPIC_API_KEY` is set in your environment, the hook unsets it
for the summarizer call so `claude -p` uses your claude.ai subscription
login instead of API credits.
