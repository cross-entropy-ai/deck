# Automatic Update Check — Design

**Status**: draft
**Date**: 2026-04-17

## Goal

Check the GitHub `cross-entropy-ai/deck` repo periodically for newer releases. When a newer version exists, show an inline banner above the sidebar's footer hints and offer a one-click `upgrade` action that runs `brew upgrade cross-entropy-ai/tap/deck` in the right pane.

## Non-goals

- Supporting install methods other than Homebrew (cargo / raw binary users see a notice pointing to alternatives but no automated upgrade path).
- Delta downloads, in-place binary replacement, or any post-upgrade restart handling (user quits and relaunches deck themselves).
- Subscribing to beta / pre-release channels. Pre-releases (`0.2.0-beta`) compare *below* stable `0.2.0` under semver, so stable users are never prompted to install one. A beta subscription toggle is deferred to a future iteration.
- Notifying about security releases specifically or rendering changelog content.
- A cancellation/rollback flow for a partially-applied brew upgrade.

## Scope

### Lifecycle

- Check on startup, then every 24 hours while deck is running.
- Cache the last-check result so startups within 24 h of a previous check do *not* re-hit the network — they display the banner straight from cache if the cached result indicates an update is available.
- Feature is enabled by default. A Settings toggle (`Enabled` / `Disabled`) flips it. Disabled → no spawn, no network, no banner.

### UI surface

- **Banner** (sidebar footer, one extra row above the hints line): `v{latest} available (current v{current})   upgrade`. `upgrade` is rendered in `theme.accent`, the rest in `theme.dim`. Only rendered when an update is available *and* focus is on the sidebar.
- **Keybinding**: new `Command::TriggerUpgrade`, default key `u`, goes through the existing keybindings system and is rebindable.
- **Mouse**: clicking the `upgrade` span also triggers the upgrade.
- **Settings entry**: row 7 in the Settings page. Label `Update check`, value `Enabled` / `Disabled`. Help text shows `· last checked Nh ago` suffix when cache is present.

### Upgrade execution

- Runs `brew upgrade cross-entropy-ai/tap/deck` in a freshly-spawned PTY whose output is rendered in the right pane.
- A new `MainView::Upgrade` variant (distinct from `MainView::Plugin(idx)`) owns this single PTY.
- When the child exits, deck returns to `MainView::Terminal` and clears both the upgrade PTY and `update_available`.
- Pressing `Esc` while in `MainView::Upgrade` sends SIGTERM to the child and returns to `MainView::Terminal`.
- If `which brew` fails before spawning, the upgrade is aborted and a centered `WarningState::Proactive` popup surfaces with install instructions.

## Architecture

### New module: `src/update.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub latest_version: String,     // "0.2.0" — "v" prefix stripped before storage
    pub current_version: String,    // env!("CARGO_PKG_VERSION") snapshot
    pub release_url: String,        // html_url from GitHub API
    pub checked_at: u64,            // unix seconds
}

pub struct UpdateChecker {
    tx: Sender<UpdateRequest>,
    rx: Receiver<UpdateResult>,
    _handle: JoinHandle<()>,
}

enum UpdateRequest { Check, Shutdown }

pub enum UpdateResult {
    /// Network call succeeded and parsed cleanly. `status` contains the
    /// latest release metadata regardless of whether it's newer than current.
    Ok { status: UpdateStatus, newer_than_current: bool },
    /// Call failed (offline, timeout, rate limit, parse error). Main loop
    /// logs one stderr line and leaves UI state unchanged.
    Err(String),
}

impl UpdateChecker {
    pub fn spawn() -> Self;
    pub fn request(&self, req: UpdateRequest);
    pub fn try_recv(&self) -> Option<UpdateResult>;
}

pub fn compare(current: &str, latest: &str) -> Option<bool>;  // Some(true) if latest > current
```

### Worker thread behavior

On `UpdateRequest::Check`:

1. `reqwest::blocking::Client::builder().user_agent("deck/<version>").timeout(Duration::from_secs(5)).build()`
2. `GET https://api.github.com/repos/cross-entropy-ai/deck/releases/latest`
3. Accept header `application/vnd.github+json`.
4. Parse JSON via serde. Expect at least `tag_name` (String) and `html_url` (String).
5. Strip a leading `v` from `tag_name`; semver-parse both it and `CARGO_PKG_VERSION`.
6. Build `UpdateStatus`; send `UpdateResult::Ok { status, newer_than_current: latest > current }`.

Any failure (network error, non-2xx, missing field, invalid semver) → send `UpdateResult::Err(message)` and keep the thread alive.

`UpdateRequest::Shutdown` drops the receiver; thread exits. The `App` fires this on `Quit`.

### App integration

New fields on `App`:

```rust
update_checker: Option<UpdateChecker>,    // None when config says Disabled
update_available: Option<UpdateStatus>,   // Banner shown iff Some
last_update_check: Option<Instant>,       // Schedules the 24h retry
```

Startup order (inside `App::new`):

1. Load `Config`.
2. If `config.update_check == Enabled`:
   a. `UpdateCache::load()` — tolerates missing / corrupt files silently.
   b. If cache present and fresh (<24 h old), treat it like a freshly-received result:
      - Parse cache's `latest_version` vs current; if newer → `update_available = Some(status)`.
      - Set `last_update_check` to `Instant::now() - (now - checked_at)` so the 24 h retry lands at the right time.
   c. If cache missing or stale → spawn `UpdateChecker`, send one `Check`, set `last_update_check = Instant::now()`.
3. If `Disabled` → none of the above runs.

Main loop (post-render, post-event): `if let Some(result) = checker.try_recv()` → handle one of:

- `Ok { status, newer_than_current }`:
  - Always write cache (`UpdateCache::save`).
  - If `newer_than_current` → `update_available = Some(status)`; else `update_available = None`.
- `Err(msg)`: `eprintln!("deck: update check failed: {}", msg)`, leave `update_available` alone, do not write cache (so next startup retries).

Periodic retry: if `last_update_check.elapsed() >= Duration::from_secs(24 * 3600)` and `update_checker` is present, send `Check` and reset the timer. Placed next to the existing `REFRESH_INTERVAL` check in the main loop.

### Cache file

Path: `~/.config/deck/update-cache.json`.

Schema: exactly `UpdateStatus` (one JSON object). Example:

```json
{
  "latest_version": "0.2.0",
  "current_version": "0.1.3",
  "release_url": "https://github.com/cross-entropy-ai/deck/releases/tag/v0.2.0",
  "checked_at": 1713350400
}
```

- **Separate from `config.json`** because (a) `config.save()` is triggered by frequent UI events and coupling cache writes with those would be awkward, (b) users syncing dotfiles shouldn't ferry a stale cache, (c) a corrupt cache shouldn't break config load.
- Write only on successful checks. Failed checks intentionally leave the previous cache in place so that a freshly-online launch after 24 h of connectivity-gap still re-checks.

## Data flow

```
┌─────────────┐   Check          ┌─────────────────┐
│  App main   │ ───────────────> │ UpdateChecker   │
│  loop       │                  │ worker thread   │
│             │ <─────────────── │                 │
│             │   UpdateResult   │ reqwest 5s t/o  │
│             │                  │  + semver       │
│             │                  └─────────────────┘
│             │
│  render()   │ ──> banner iff update_available.is_some() && sidebar focus
│  dispatch() │ ──> Action::TriggerUpgrade ──> spawn upgrade PTY
└─────────────┘
```

## Config changes

### `Config`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdateCheckMode {
    #[default]
    Enabled,
    Disabled,
}

pub struct Config {
    // ... existing fields ...
    pub update_check: UpdateCheckMode,
}
```

- `#[serde(default)]` on `Config` already covers the absent-field case.
- Default `Enabled`.
- Existing `ensure_complete` pass (used for keybinding backfill) is untouched — top-level scalar fields already round-trip on every `save()`.

## Keybindings additions

- Add `Command::TriggerUpgrade` to `Command` enum.
- `name() → "trigger_upgrade"`, `description() → "upgrade deck"`.
- `default_keys() → [KeyBinding::new(KeyCode::Char('u'), KeyModifiers::NONE)]`.
- Appears in `Command::ALL`, therefore participates in conflict detection + `ensure_complete` backfill.
- `command_to_action(Command::TriggerUpgrade) → Action::TriggerUpgrade`.
- In `sidebar_key_to_action`, the existing dispatch through `state.keybindings.lookup(key)` already covers it — no special casing.

## Action additions

```rust
pub enum Action {
    // ... existing ...
    TriggerUpgrade,
    AbortUpgrade,     // Esc while in MainView::Upgrade
}
```

`TriggerUpgrade` is handled in `App::dispatch` (not `apply_action`) because it spawns a PTY:

```rust
Action::TriggerUpgrade => {
    if self.state.update_available.is_none() {
        return false;   // no-op if no update waiting
    }
    if !has_brew() {
        self.warning_state = Some(WarningState::Proactive {
            text: "Homebrew not found",
            detail: "Install from https://brew.sh, then retry.\n\
                     Alternatively: cargo install --git https://github.com/cross-entropy-ai/deck".into(),
        });
        return false;
    }
    match self.spawn_upgrade_pty() {
        Ok(()) => {
            self.state.main_view = MainView::Upgrade;
            self.state.focus_mode = FocusMode::Main;
        }
        Err(e) => eprintln!("deck: failed to spawn upgrade: {}", e),
    }
    false
}
```

`AbortUpgrade` (also in `dispatch`): if `upgrade_instance.is_some()`, send SIGTERM to its PID, drop the instance, set `main_view = Terminal`.

## MainView addition

```rust
pub enum MainView {
    Terminal,
    Settings,
    Plugin(usize),
    Upgrade,
}
```

`App` gains `upgrade_instance: Option<PluginInstance>` — reusing `PluginInstance` verbatim since it already encapsulates `pty / parser / alive`.

Render branch in `render()`: mirror the `Plugin(idx)` path, reading from `upgrade_instance.as_ref()`. Add title `" Upgrading deck "` on the bordered block so users know what's running.

Main-loop exit detection: same pattern as the plugin alive check:

```rust
if self.state.main_view == MainView::Upgrade {
    if self.upgrade_instance.as_ref().is_some_and(|inst| !inst.alive) {
        self.upgrade_instance = None;
        self.state.main_view = MainView::Terminal;
        self.state.focus_mode = FocusMode::Main;
        self.state.update_available = None;    // don't re-show banner
    }
}
```

`key_to_action` top-level branch: when `main_view == MainView::Upgrade`, `Esc` returns `Action::AbortUpgrade`; all other keys are forwarded to the upgrade PTY (reuse `pty::encode_key` path).

## Brew detection

```rust
fn has_brew() -> bool {
    std::process::Command::new("which")
        .arg("brew")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
```

Lives in `update.rs` as a free function or on `App`. Called before `spawn_upgrade_pty`; on miss, the `WarningState::Proactive` path described above fires.

## UI / rendering changes

### Sidebar footer

The footer constraint goes from a fixed `Constraint::Length(3)` to dynamic:

```rust
let footer_height = if should_show_banner { 4 } else { 3 };
```

`should_show_banner = sidebar_active && state.update_available.is_some() && area.width >= BANNER_MIN_WIDTH`.

`BANNER_MIN_WIDTH = 30` — if sidebar is narrower, skip the banner entirely rather than truncate mid-word.

When `should_show_banner` is true, the banner row is inserted immediately above the hints row inside `draw_footer`. A small helper:

```rust
fn draw_banner(width: usize, status: &UpdateStatus, theme: &Theme) -> Line<'static>;
```

returns:

```rust
Line::from(vec![
    Span::raw(" "),
    Span::styled(format!("v{} available (current v{})",
        status.latest_version, status.current_version),
        Style::default().fg(theme.dim)),
    Span::raw("   "),
    Span::styled("upgrade", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
])
```

The caller records the column range occupied by `upgrade` for mouse hit-testing.

### Footer row layout

Rows inside the footer area, top to bottom:

| Row | Content                                           |
| --- | ------------------------------------------------- |
| 1   | separator (`─` fill)                              |
| 2   | banner (only when `should_show_banner`)           |
| 3   | hints line 1                                      |
| 4   | hints line 2 (overflow) OR About (help mode)      |

Total footer height is either 3 (no banner) or 4 (banner present).

Priority when things compete for row 4:

- If hints overflow onto a second row → row 4 = hints line 2, About is dropped.
- Else if help is visible → row 4 = About.
- Else → row 4 is blank.

Because `footer_height` is decided before `Layout::vertical` splits the sidebar, we pre-compute wrap requirements using `pack_hint_lines(entries, width, theme)` on the caller side (already exists from the prior feature). Combined decision logic:

```rust
let banner = should_show_banner(state, area.width);
let footer_height: u16 = if banner { 4 } else { 3 };
```

Inside `draw_footer`, rows 3-4 are filled from the packed hint lines first, then the About line takes whichever row remains if any.

### Mouse hit-testing

`AppState::banner_upgrade_at(col: u16, row: u16) -> bool` — returns true if the click lands on the `upgrade` span. Uses state the sidebar rendering stores about the banner's current position (`AppState` gains small `banner_bounds: Option<Rect>` updated per render, like how `context_menu` already stores its x/y).

`mouse_to_action` gets a new branch near the top:

```rust
if mouse.kind == MouseEventKind::Down(MouseButton::Left) && state.banner_upgrade_at(mouse.column, mouse.row) {
    return Action::TriggerUpgrade;
}
```

### Settings page (row 7)

- `SETTINGS_ITEM_COUNT: usize = 7` (was 6 after the keybindings feature).
- New entry in the `entries` vec in `draw_settings_page`:

```rust
(
    "Update check",
    if settings.update_check_enabled { "Enabled" } else { "Disabled" }.to_string(),
    &settings.update_check_help,   // owned String assembled in app.rs
),
```

- `SettingsView` gets `pub update_check_enabled: bool` and `pub update_check_help: String`.
- `update_check_help` is built in `app.rs::render` as:
  - `"Left/right toggles auto update check"` when no cache
  - `"Left/right toggles auto update check · last checked {N}h ago"` when cache present (round to nearest hour, floor at "just now" for <1 h)
- `SettingsAdjust` dispatch adds `6 => apply_action(state, Action::ToggleUpdateCheck)`.
- New `Action::ToggleUpdateCheck` (in `apply_action`) flips `state.update_check_mode`, sets `fx.save_config = true`, and — crucially — does NOT spawn the worker or tear it down directly. The next `main loop` tick observes the mode change and adjusts (see below).

### Enabled→Disabled / Disabled→Enabled at runtime

- `App` reads `self.state.update_check_mode` each tick. State transitions handled once per tick:
  - Was `None`, now `Enabled` → call the same startup spawn logic (cache lookup, maybe send Check).
  - Was `Some`, now `Disabled` → send `UpdateRequest::Shutdown`, drop the checker, clear `update_available`.
- This keeps `apply_action` pure and side-effect-free (per existing project convention).

## Cargo dependencies

```toml
reqwest = { version = "0.12", default-features = false, features = ["blocking", "json", "rustls-tls"] }
semver = "1"
```

- `rustls-tls` avoids linking system OpenSSL (keeps cross-platform builds simpler).
- No `tokio` in `[dependencies]` — `reqwest::blocking` pulls it transitively but we never touch async code.

**Cost acknowledgment**: reqwest pulls in tokio + hyper + rustls, roughly +1.5 MB binary and notably slower cold builds. Accepted per user decision.

## Testing

### `update.rs` unit tests

- `parse_release_json` — valid payload → `tag_name` + `html_url` extracted; missing field → Err; tag without `v` prefix → still works.
- `compare("0.1.3", "0.2.0") == Some(true)`
- `compare("0.2.0", "0.2.0") == Some(false)`
- `compare("0.3.0", "0.2.0") == Some(false)` (user on nightly)
- `compare("0.2.0", "0.2.0-beta.1") == Some(false)` (pre-release < stable)
- `compare("garbage", "0.2.0") == None`

### `UpdateCache` tests

- Round-trip serialize/deserialize.
- `load` from a path that doesn't exist → `None`, no panic.
- `load` from a file containing invalid JSON → `None`, no panic (log a line to stderr).
- `is_fresh(now, cache, ttl)` boundary: exactly `ttl` old → false (stale); `ttl - 1s` → true.

### `keybindings.rs` additions

- `trigger_upgrade` appears in `Command::ALL` and has default key `u`.
- `ensure_complete` backfills it.
- `Command::from_name("trigger_upgrade") == Some(TriggerUpgrade)`.

### `action.rs` integration tests

- `Action::TriggerUpgrade` when `update_available.is_none()` → no state change, no warning.
- `Action::TriggerUpgrade` when `update_available.is_some()` + mock `has_brew()` returns false → `warning_state` is `Some(Proactive { ... "Homebrew" ... })`, `main_view == Terminal`.
- `Action::TriggerUpgrade` happy path (can only be validated in a full integration test where PTY spawn is real) — covered by manual QA instead.
- `Action::ToggleUpdateCheck` flips mode and signals `save_config`.

### `config.rs` serde tests

- `parse_json_with_update_check_enabled`
- `parse_json_with_update_check_disabled`
- `parse_json_without_update_check_uses_enabled_default`
- Roundtrip.

### Manual QA checklist

1. Default config, online launch → if GitHub has newer release, banner appears; restarts within 24 h don't re-hit network.
2. `rm ~/.config/deck/update-cache.json` → next launch fetches fresh.
3. Settings → Update check → flip to Disabled → banner disappears; quitting and relaunching doesn't spawn the worker (watch for absence of network traffic or stderr logs).
4. Press `u` with update available → right pane shows `brew upgrade` output; on success, pane returns to Terminal after brew completes.
5. `PATH=/tmp` (no brew) launch, press `u` → centered warning, no spawn.
6. Kill network mid-session, wait 24 h → `eprintln` error line; UI unchanged.
7. Launch offline with cache indicating an update → banner still shows (cache-backed).
8. Hand-edit cache `latest_version = "99.0.0"` → relaunch → banner says `v99.0.0 available`.
9. In upgrade view, press `Esc` → SIGTERM to brew child, return to Terminal.
10. Rebind `trigger_upgrade: "U"` in config → `u` no longer triggers, `U` does. Existing keybinding-conflict detection flags `u` if user binds it elsewhere.
11. Narrow sidebar (<30 cols) with update available → banner skipped, footer reverts to 3 rows.

## File-level change summary

- **New** `src/update.rs` — `UpdateChecker`, `UpdateStatus`, `UpdateCache`, `parse_release_json`, `compare`, `has_brew`.
- `src/config.rs` — add `UpdateCheckMode` enum and `update_check` field on `Config`; serde tests.
- `src/state.rs` — add `update_check_mode: UpdateCheckMode`, `update_available: Option<UpdateStatus>`, `banner_bounds: Option<Rect>`; bump `SETTINGS_ITEM_COUNT` to 7; new `AppState::banner_upgrade_at`.
- `src/action.rs` — new `Action::TriggerUpgrade`, `Action::AbortUpgrade`, `Action::ToggleUpdateCheck`; handle `ToggleUpdateCheck` in `apply_action`; dispatch the other two in `App::dispatch`; extend `SettingsAdjust` for row index 6; mouse branch for banner click.
- `src/keybindings.rs` — add `Command::TriggerUpgrade` everywhere (name, description, default key, ALL).
- `src/ui.rs` — footer height made dynamic; `draw_banner`; extend `SettingsView` with `update_check_enabled` + `update_check_help`; new Settings row render; banner-bounds capture into `AppState` during render.
- `src/app.rs` — construct `UpdateChecker` conditionally; worker plumbing, receive loop, 24h retry, enable/disable transitions; `spawn_upgrade_pty`, `abort_upgrade`, brew detection; `MainView::Upgrade` render path; on-exit cleanup.
- `src/main.rs` — add `mod update;`.
- `Cargo.toml` — `reqwest` + `semver`.

## Open questions / deferred

- **Beta channel subscription**: skipped in v1. Would need a `beta_channel: bool` config flag plus changes to `compare` (or switch to `include_prerelease` semver behavior).
- **Alternate upgrade commands** (cargo, binary tarball): v1 only ships brew path. A future refactor could detect install method via the binary's `$CARGO_HOME`/`/usr/local/bin` path and pick the right command.
- **Rate-limit handling**: GitHub unauthenticated gives 60/h per IP; even with pathological restart loops we won't exceed that because of the 24h cache. No special handling beyond treating HTTP 403 as a normal `UpdateResult::Err`.
- **User-Agent / telemetry**: the request sends `deck/{version}` as User-Agent. No telemetry beyond what GitHub's access logs already record.
