# Readout modes: verbosity, per-scope control, and more-frequent updates — plan

Status: **planned, not implemented.** Turns today's single summarize-or-verbatim
behavior into named, quickly-adjustable modes that are scoped per repo and per
session, with per-mode prompts and content-aware handling. Sibling plans:
[tray-app.md](tray-app.md), [meta-agent.md](meta-agent.md).

## Today

`claude/vox-speak.sh` (the Stop hook) has one verbosity control: `verbatim_max`
(responses at/under N chars are spoken as-is; longer ones get a 1–3 sentence
Haiku summary) plus a single `summary_prompt`. Config already cascades
**`.vox.json` (per-repo) → `state.json` (global) → built-in default**, and every
repo the hook has spoken from is recorded in `projects.json`. This cascade is
the foundation; we extend it rather than replace it.

## Mode ladder

A single ordered ladder so "more/less verbose" is one step up/down:

| Mode | Summarizer | Speaks | Extra hooks |
|---|---|---|---|
| `off` | — | nothing | — |
| `brief` | yes (headline) | one-line outcome only | — |
| `summary` *(default)* | yes | 1–3 sentences (today's behavior) | — |
| `detailed` | yes | longer structured summary (steps + outcome), bigger word budget | narrate tool actions |
| `verbatim` | no (light cleanup only) | the message via `md2speech`, **but diffs/large code still summarized** | narrate tool actions |

Each mode is a preset bundle of: summarizer on/off, word budget, prompt
template, tool-narration on/off, and content rules. Presets live in code;
config overrides them.

## Config backbone

Extend the JSON at every cascade level (`state.json`, `.vox.json`, and a new
session layer):

```jsonc
{
  "mode": "summary",                 // ladder value
  "modes": {                         // optional per-mode overrides
    "detailed": { "prompt": "…", "max_words": 120, "narrate_tools": true },
    "verbatim": { "summarize_diffs": true }
  }
}
```

**Resolution order (new session layer on top):**

```
session override  →  repo .vox.json  →  global state.json  →  built-in preset
```

- **Session layer** is new: per-`session_id` overrides (the hook receives
  `session_id` on stdin) in a small runtime file, e.g.
  `~/.claude/vox/sessions/<session_id>.json`, pruned on `SessionEnd`. Lets you
  say "make *this* session verbatim" without touching repo or global defaults.
- Legacy `"summary_prompt"` and `"verbatim_max"` remain valid and map onto the
  new model (back-compat).

**Shared resolution — avoid drift.** The hook is bash+jq; the tray and TUI are
Rust. To keep one source of truth, add a resolver to the `vox` binary
(e.g. `vox config resolve --cwd … --session …` prints the effective config as
JSON) and have the hook call it instead of re-implementing the cascade in jq.
All three surfaces then agree by construction.

## Quick adjustment (parity across surfaces)

The repo's parity rule applies — every control exists in all three places:

- **`/vox` skill**: "vox verbatim", "vox brief", "vox more detailed", and
  relative "vox more" / "vox less" (step the ladder). Each writes to a chosen
  **scope**: this session (default for a spoken quick-tweak), this repo, or
  global. The skill already edits `state.json`; extend it to target scope.
- **Tray**: a **Mode** submenu (radio items on the ladder) + optionally a global
  shortcut to cycle verbosity; a Mode control in the Settings tab with the same
  scope picker used for other per-repo overrides.
- **TUI**: a Mode field in Tab-settings.

## More-frequent updates (the "read more of it" ask)

Stop fires once per turn. To narrate mid-turn we add hooks — but note the hard
limit from Claude Code: **no per-token streaming via hooks.** Finest achievable
granularity is per-assistant-message (Stop) + per-tool-call (PostToolUse). See
[the hook rundown in meta-agent.md](meta-agent.md#claude-code-hook-facts).

Plan:

- `detailed`/`verbatim` modes enable:
  - **`PostToolUse`** (matched to Edit/Write/Bash, `async`) → short spoken
    action notes ("editing tui.rs", "ran the tests — 3 passed").
  - **`Notification`** → speak permission prompts and idle notices.
- **Install-all, gate-by-mode:** install every hook script once and have each
  script read the resolved mode and **no-op unless the mode wants that
  granularity.** Switching modes is then a pure config edit — no re-running the
  installer, no `settings.json` churn. (Today `install.sh` only registers the
  Stop hook; it would additionally register the PostToolUse/Notification
  entries, all pointing at scripts that self-gate.)
- Rate-limit tool narration (debounce, collapse bursts) so a 20-edit turn
  doesn't become 20 utterances; the readout queue (v0.8.0) already serializes
  playback.

## Content-aware handling

Even in `verbatim`, some content shouldn't be read literally:

- **Diffs / large code blocks / tables** → collapse to a spoken summary
  ("changed 3 files, added retry logic in providers/mod.rs") instead of reading
  syntax. `md2speech.sh` already turns fenced code into "Code block omitted." —
  extend it to (a) detect unified diffs and (b) optionally produce a one-line
  summary (heuristic first: files touched + net +/- lines; optional cheap Haiku
  pass for the code portion only).
- **Context tags (optional)**: prepend a short spoken tag so audio-only makes
  sense — "in vox: …" or "editing tui.rs: …" — derived from `cwd`/tool input,
  no LLM.

## Open questions

- Session-layer lifetime: prune on `SessionEnd` only, or also TTL?
- Does "per application" mean per repo, per `session_id`, or per Claude Code
  instance? Current plan treats repo and session as the two scopes; revisit if
  "app" should mean something else (e.g. different tools driving vox via the
  socket).
- Tool-narration verbosity: which tools are worth announcing by default, and
  how aggressively to debounce.
- Whether `vox config resolve` should be the resolver for the tray/TUI too
  (recommended) or only the hook.

## Phasing

1. Mode presets + config backbone + `vox config resolve`; hook consumes it
   (behavior parity with today at `summary`).
2. Session layer + scope-aware `/vox` skill; ladder up/down.
3. Tray/TUI Mode controls (parity).
4. PostToolUse/Notification narration (install-all, gate-by-mode) with
   debouncing.
5. Content-aware diff/code summarization + context tags.
