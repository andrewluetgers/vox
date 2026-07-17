# Tray app: real window, About/version, and provider discovery — plan

Status: **planned, not implemented.** Covers three panel/tray gaps: (1) the
panel isn't a proper app window, (2) there's no version/About anywhere, (3)
cloud providers are invisible unless a key is already set. Sibling plans:
[readout-modes.md](readout-modes.md), [meta-agent.md](meta-agent.md).
Provider design: [providers.md](providers.md).

## 1. Make the panel a proper application window

**Cause (confirmed in code):** `tray/src/main.rs` `setup()` sets
`app.set_activation_policy(ActivationPolicy::Accessory)` — accessory apps have
no Dock icon, no menu bar, and no ⌘-Tab entry. The panel window
(`open_panel_tab`) is also built with a title and size but **no application
menu**, so ⌘C/⌘V/⌘A/⌘W don't work in its text fields.

**Decision (chosen): always in the Dock.** Use
`ActivationPolicy::Regular` unconditionally — vox-tray presents as a normal app
(Dock icon, menu bar, ⌘-Tab) while still owning a menu-bar (tray) item. This
drops the pure menu-bar-only feel; accepted.

Plan:

- **Activation policy** → `Regular` in `setup()`. The status-bar tray item
  stays as-is.
- **Application menu** via `MenuBuilder` + `PredefinedMenuItem`:
  - **App menu**: About vox (opens the About view, §2), Hide, Quit.
  - **Edit menu**: Undo, Redo, Cut, Copy, Paste, Select All — the predefined
    items wire the standard ⌘ shortcuts so panel text fields behave natively.
  - **Window menu**: Minimize, Zoom, Close.
- **App/Dock icon**: wire `tray/icons/icon.png` as the bundle/window icon (a
  higher-res icon may be worth adding for Dock/⌘-Tab crispness).
- **Window behavior**: intercept close → **hide** the window (keep the socket,
  tray, and shortcuts alive) rather than terminating; set a sensible min size;
  center on first open; remember size/position across opens.

Scope: contained to the tray crate, no new deps.

## 2. Version + About

Nothing surfaces a version today (no `CARGO_PKG_VERSION` use, no version in the
socket `status`). vox-tray is `0.1.0`; the `vox` engine is separately versioned
(`vox --version` → `vox 0.8.0`).

Plan:

- New Tauri command `about()` returning:
  ```json
  { "tray_version": "0.1.0", "vox_version": "0.8.0",
    "vox_path": "/Users/…/.local/bin/vox" }
  ```
  `tray_version` from `env!("CARGO_PKG_VERSION")`; `vox_version` by parsing
  `vox --version`; path from the resolved binary.
- Also expose `version` over the **socket API** (`{"cmd":"version"}`) for parity
  with the CLI, and add it to the `status` reply.
- **UI**: an About section — simplest is a footer block at the bottom of
  Settings ("vox-tray 0.1.0 · vox 0.8.0 · Kokoro-82M") with repo/license links;
  optionally a dedicated About window opened from the app menu and a tray
  "About vox" item.

## 3. Cloud-provider discovery in Settings

**Problem:** with no cloud key set, the provider groups in the voice picker are
empty, so the selector effectively shows only Kokoro and the whole
cloud-provider capability is undiscoverable. We want to advertise the
capability regardless of key state, and explain how to enable it — **without
ever storing a secret in the app.**

Plan — a persistent **Cloud providers** block in the Settings tab (near the
voice selector, in/under the readouts section):

- One row per provider (OpenAI, ElevenLabs, xAI, Groq), each showing:
  - provider name + a status chip: **`OPENAI_API_KEY` ✓ detected** or
    **`OPENAI_API_KEY` — not set** (env var name styled as inline code; the
    value is never read back or shown);
  - a one-liner of what it adds (e.g. "ElevenLabs — widest voice variety").
- Explanatory copy: *"vox reads provider keys from environment variables at
  synthesis time and never stores them. Add one of the variables above with
  your token to enable that provider."* with a copy-ready example styled as a
  code block:
  ```sh
  export ELEVENLABS_API_KEY="…"
  ```
- **Voice picker**: keep provider grouping, but render locked providers as a
  disabled optgroup with a hint (`OpenAI — set OPENAI_API_KEY`) instead of
  omitting them, so cloud voices are discoverable from the dropdown too.

**Data source**: `vox --list-voices --json` already returns
`{path, provider, provider_label, ready}` per voice; extend the tray's
`list_voices` (or add `providers()`) to return, per provider, its `env_var` and
detected/available state. Env-var names are fixed per provider
(see [providers.md](providers.md)).

### The launchd environment caveat (important)

vox-tray is started by a **LaunchAgent**, so it inherits **launchd's**
environment, not the user's shell. A key exported in `~/.zshrc` will **not** be
visible to the tray (or to any `vox` it spawns). The discovery UI must reflect
what the app actually sees and explain the fix. Options, with trade-offs:

- `launchctl setenv OPENAI_API_KEY …` — visible to GUI apps this login session;
  **not persistent** across reboots.
- Add to the LaunchAgent plist `EnvironmentVariables` — persistent, but that
  **writes the secret into a plist file**, contradicting "no secrets stored."
  Reject, or make it an explicit opt-in with a clear warning.
- A user-owned env file (e.g. `~/.config/vox/env`) that the tray sources on
  launch — persistent, user-controlled, outside the app bundle. Likely the best
  fit for the "no secrets in the app" principle; document it.

Detect the effective state (does the running process see the var?) and, when a
key is set in the shell but not seen by the app, show a targeted hint about the
launchd nuance rather than a generic "not set."

## Naming note

The Settings section is currently "**Readouts**." It's a slightly awkward label
for what is really speech/voice output config. Open question: keep "Readouts",
or rename to "Speech" / "Voice & speech". Low stakes; decide during
implementation.

## Open questions

- Dedicated About window vs. a Settings footer block — start with the footer.
- Provider rows: static list vs. driven entirely by `vox --list-voices --json`
  (prefer the latter so the CLI stays the single source of truth).
- Where the env-file convention (`~/.config/vox/env`) lives and whether `vox`
  (CLI) should also source it for consistency with the tray.

## Phasing

1. Activation policy `Regular` + app menu (Edit menu is the high-value bit) +
   close-to-hide.
2. `about()`/`version` command + Settings footer.
3. Cloud-provider discovery block + voice-picker locked-group rendering +
   launchd-env detection/hint.
