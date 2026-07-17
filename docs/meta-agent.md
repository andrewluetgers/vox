# Meta-agent: conversation about the conversation — plan

Status: **planned, needs a spike.** A voice-first "observer" you can ask about
the current Claude Code session — "what's it doing?", "summarize progress",
"any risks?" — answered by a headless Claude that reads the session, spoken
back through vox. Sibling plans: [tray-app.md](tray-app.md),
[readout-modes.md](readout-modes.md).

## Concept

Rather than vox maintaining its own running context from a hook stream, we
**instrument Claude over its own `.claude` session**: capture which session is
live, and on demand spin up a headless Claude that reads that session and
answers questions. This is a meta-harness — a high-level agent watching the
work — without us re-deriving context.

## Decision: read-only session access

**Chosen: read-only.** Feed an exported/transcript view of the session to a
**fresh** `claude -p` observer; never `--resume` the live session. Rationale:
`--resume <id>` gives the fullest context but risks appending to / forking the
session you're actively driving. Read-only keeps the observer strictly a
spectator.

- **Preferred source:** `/export` (human-readable, officially supported) or a
  defensively-parsed read of the transcript JSONL (schema is officially
  internal and may change between versions — never hard-depend on it).
- **Observer invocation:** `claude -p --bare` with a system prompt like *"You
  observe a Claude Code coding session. Given the transcript below, answer the
  user's question about progress, state, and risks. Be concise and spoken-word
  friendly."* plus the exported transcript, `--output-format json`, result
  spoken via vox. Unset `ANTHROPIC_API_KEY` so it uses the subscription login
  (same pattern as `vox-speak.sh`).

## Session tracking

- A **`SessionStart`** hook records `{ session_id, transcript_path, cwd, ts }`
  per project into e.g. `~/.claude/vox/sessions/index.json`; refreshed by later
  hooks (Stop/PostToolUse all carry these fields). `SessionEnd` marks it closed.
- Given a repo, the meta-agent picks the most recent live session for that repo;
  when ambiguous (multiple sessions), it asks or lists them.

## Interaction (v1)

Typed question in, spoken answer out (real speech-to-text is a **separate
feature**, out of scope here):

- **Entry points**: the tray panel Speak box gains an "Ask about this session"
  toggle; the `/vox` skill ("vox, what's it working on?"); a global shortcut
  that prompts for a question.
- **Flow**: question → resolve target session → export/read transcript →
  `claude -p` observer → speak the answer, and log it to history as a
  distinct source (`source: "meta"`) so it's filterable alongside readouts.

## Phase 2 (optional): proactive observer

A longer-running watcher for "an agent watching everything I do":

- The Stop/PostToolUse hooks append events to a per-session log; a background
  process (could live in vox-tray) periodically produces a **rolling summary**
  and speaks **milestones** — "tests just went green", "it's been editing the
  same file for 10 minutes", "it's waiting on your approval."
- Milestone detection is heuristic first (tool patterns, timings, notification
  events); LLM summarization only at intervals to control cost/latency.

## <a id="claude-code-hook-facts"></a>Claude Code hook facts (basis for these plans)

From an authoritative rundown against the installed Claude Code (v2.1.207);
treat the finer-grained events as **to-be-verified against `/hooks`** before
building on them, and treat the transcript schema as unstable.

- **Hooks fire at discrete points — there is no per-token streaming hook.**
  Finest granularity: per assistant message (`Stop`) + per tool call
  (`PostToolUse`). Token streaming exists only in headless
  `claude -p --include-partial-messages`, which doesn't fit an
  already-interactive session.
- **Frequency ladder (stable core):** `SessionStart`/`SessionEnd` (per session),
  `UserPromptSubmit` (per prompt), `PreToolUse`/`PostToolUse` (per tool call),
  `Notification` (permission/idle), `Stop` (per turn, full final message),
  `SubagentStop`, `PreCompact`.
- **Common stdin fields on every hook:** `session_id`, `transcript_path`,
  `cwd`, `hook_event_name`, `permission_mode`. Event-specific: `Stop` carries
  the final assistant message (our hook reads it from `transcript_path` today,
  which is robust); `PostToolUse` carries tool name/input/result;
  `UserPromptSubmit` carries the prompt text; `Notification` carries the
  notification text; `SessionStart` carries `source`
  (startup/resume/clear/compact).
- **Transcript:** `~/.claude/projects/<slug>/<session-id>.jsonl`, where `<slug>`
  is the cwd with non-alphanumerics → `-`. JSONL, one entry per line. **Schema
  is internal and version-specific** — prefer `/export`; if tailing, parse
  defensively and expect breakage across versions.
- **Headless / meta options:** `claude -p` (print/headless),
  `--output-format json|stream-json`, `--resume <id>` / `--continue`,
  `--bare`, `--append-system-prompt`, and the Claude Agent SDK for a programmatic
  observer. For v1 read-only observer, plain `claude -p --bare` over an export
  is the least-coupled choice.
- **Hook mechanics we already use / will use:** `matcher` (filter by tool
  name/notification type), `async` (non-blocking), `timeout`,
  `statusMessage`.

## Open questions

- Export latency and cost per query — is `/export` fast enough for a
  conversational feel, or do we cache/incrementally update a working summary?
- Multi-session disambiguation for a repo (pick latest, or list and ask).
- Whether Phase 2's proactive observer belongs in vox-tray (already
  long-running, owns the socket) or a separate daemon.
- STT for true voice input — deferred; design the v1 pipeline so a speech
  front-end can drop in later.

## Phasing

1. `SessionStart`/`SessionEnd` session index.
2. Read-only observer over `/export` + one entry point (tray Speak box "Ask
   about this session"), answers spoken + logged as `source: "meta"`.
3. `/vox` skill + global-shortcut entry points; multi-session handling.
4. (Optional) Phase-2 proactive observer with milestone announcements.
