# Code review & cleanup plan

> Full-codebase review (2026-06, v0.9.2 + working tree). Supersedes
> `high-roi-cleanup.md`: its Phase 1 (Effect enum), Phase 2
> (NewSessionTarget), and Phase 3 (SessionControl) have landed; its
> Phase 4 (extract managers) is absorbed into Phase 4 below.
>
> Line numbers reference the tree at review time and will drift.
>
> **Progress legend (added 2026-06-12):** `[x]` done · `[~]` partial ·
> `[ ]` not started. Status is checked against the working tree at
> commit `8d4b0b6`.

## What's already good (don't undo)

- The `Effect` enum + `SideEffect` Vec wrapper works: reducers are
  IO-free, dispatch iterates effects in order (`state.rs:604`,
  `dispatch.rs:764`).
- `SessionControl` is load-bearing — no dead trait methods; the executor
  runs all mutating ops off the UI thread on per-backend FIFOs.
- Remote quoting is solid: every user value interpolated into ssh
  commands goes through `shell_single_quote` / `shell_quote_remote_path`;
  no injection hole was found (audited kill/rename/switch/focus/order/
  new-session/ls paths).
- The local/remote rule holds in the renderer: `&[&dyn SidebarSession]`,
  no `is_remote` branching outside the sanctioned divider/label carve-out.
- Reducer test coverage is genuinely strong (~90 tests). Clippy is clean.

---

## Part 1 — Bugs

### P0: data loss / wrong destructive action

1. [x] **`Config::load()` destroys the user's config on parse error.**
   `config.rs:266` does `confy::load_path(..).unwrap_or_default()`, then
   the summary-prompt migration reports "changed" (default version 0 < 3)
   and `config.save()` rewrites the file — remotes, keybindings, plugins
   gone after one typo. The TUI is shielded by the preflight guard, but
   `deck remote add/list/remove` (`main.rs:277,336,347`) call `load()`
   unguarded; even read-only `deck remote list` can wipe a config.
   Strict inner types amplify it: one malformed `PluginConfig` /
   `ForwardSpec` entry fails the whole file.
   **Fix:** parse errors must never reach `save()` — fall back to
   defaults *in memory only*, or route all entry points through
   `try_load` and surface the error. *(commit 95816d2)*

2. [x] **`--force` (the default) can kill an unrelated process.**
   `pid_looks_like_deck` (`tmux.rs:332`) is a substring match
   (`command.contains("deck")`) on a PID read from a stale
   `/tmp/deck.lock`; `instance_guard.rs:94` SIGTERMs then SIGKILLs it.
   Crash → PID recycled to `vim deck.md` → next `deck` kills it.
   **Fix:** match exact process basename (and/or store process
   start-time in the lock and compare). *(commit 2f11c7c)*

3. [x] **tmux `-t name` does prefix matching — kill/rename can land on the
   wrong session.** Verified live: with only `workbench` running,
   `tmux kill-session -t work` kills it. All bare-name targets are
   exposed (`tmux.rs:160-183`, `remote_tmux.rs:508-535,556-568`); the
   executor FIFO + ≤1s-stale row snapshot provides the race window.
   **Fix:** use the exact form `-t '=name'` everywhere (single-quoting
   already protects the leading `=` from zsh on remotes). *(commit c0a655e)*

4. [x] **Summary logs dump pane contents world-readable.** `summary.rs:157`
   writes every agent pane's visible buffer (tokens, env output…) to
   `/tmp/deck-summary/summary-<ms>.md`, mode 0644, one file per
   generation, never pruned.
   **Fix:** move under `~/.cache/deck/` with 0600, cap retained files,
   or gate behind a debug flag. *(commit 4979c83)*

### P1: core interactions broken

5. [x] **Sidebar hit-testing is off by one row.** The renderer allocates
   footer `2 + banner + plugins` (`ui/sidebar/mod.rs:172`); the
   hit-tester uses `3 + …` (`state.rs:1492`, whose comment claims it
   mirrors the renderer). Bottom visible session row is click-dead, and
   when the list overflows, the scroll offset used for hit-testing
   disagrees with the drawn one, shifting every click by a line.
   Cause: commit af34bbc changed the renderer only. → Phase 1 makes this
   class impossible. *(stopgap landed, commit 3c1aa89; structural fix
   landed with Phase 1 — geometry now flows from one shared formula and
   one `HitRegions` registry)*

6. [x] **deck reloads itself after every config save.** `config_mtime_seen`
   is only updated in the watcher branch (`app/mod.rs:766`); deck's own
   `save_config` (fired by ~20 actions: drags, toggles, theme-picker
   keystrokes, pf changes, startup forward Bootstrap) bumps the mtime,
   so ≤2s later `ReloadConfig` runs: kills all plugin PTYs, exits
   `MainView::Plugin`, closes the exclude editor mid-edit, flashes the
   "reload Ok" toast.
   **Fix:** refresh `config_mtime_seen` after self-saves (have
   `save_config` return/record the new mtime), keep the watcher for
   external edits. *(commit 6025cbc)*

7. [x] **Five overlays are keyboard-modal but not mouse-modal.**
   `mouse.rs:12-98` swallows mouse for confirm-kill / summary-popup /
   context-menu / new-session / port-forward, but not for **rename,
   add-remote, theme-picker, help, summary-language** (all modal in
   `keyboard.rs`). Clicks and wheel events pass through and switch
   sessions (incl. submitting ssh switches) behind the modal. Also,
   button rects (tabs/summary/banner/menu) are checked *before* the
   overlay guards, so they stay clickable through modals that do swallow
   mouse input. → Phase 2 (single modality source) is the real fix.
   *(fixed in Phase 2: `AppState::active_modal()` is resolved first by
   both mappers — all overlays now swallow mouse, button rects only fire
   with no modal up, and global keys no longer fire over the previously-
   permeable keyboard modals. The update banner is now also swallowed by
   those overlays. Parity test in `tests/unit/app/action/modality.rs`)*

8. [x] **The update-check cache is dead — a GitHub HTTPS check fires every
   startup.** `bootstrap_update_check` returns `(None, synthetic_instant)`
   on a fresh cache to skip the network, but `tick_update_check`
   (`app/update.rs:51-58`) sees `update_checker.is_none()` under
   `Enabled` and immediately spawns **and requests**.
   **Fix:** spawn lazily without requesting; request only when
   `last_update_request` has elapsed. *(commit 6025cbc)*

9. [x] **Keyboard kill bypasses the policy the context menu enforces.** The
   menu greys Kill on placeholder rows and on a host's last live session
   (`state.rs:39-43,245-262`); the `x` key path (`reduce.rs:226-232`)
   has neither check — you can confirm-kill "(no sessions)" (sends
   `ssh tmux kill-session` with a placeholder/empty name) or a host's
   last session. **Fix:** route both input paths through one kill-policy
   function. *(commit c434310)*

10. [x] **Kill pre-switch runs unconditionally.** `switch_to` is documented
    as "only when killing the currently attached session"
    (`state.rs:776`), but the reducer fills it whenever ≥2 sessions
    exist (`reduce.rs:251-266`) and dispatch executes it
    (`dispatch.rs:822`): killing a non-current row via right-click yanks
    the main view to a session adjacent to the *killed* one, and snaps a
    remote view back to local. **Fix:** fill `switch_to` only when the
    killed session is current. *(commit c434310)*

11. [x] **A single failed marker confirmation leaves a host permanently
    un-switchable, silently.** `MarkerReady` is sent at most once
    (`remote_spawn.rs:173-179`); if `wait_for_client_marker` misses its
    ~2s window (slow shell startup, cold connection), `marker_ready`
    stays false forever: every click parks in `pending_remote_switch`
    and never fires, with no retry and no UI signal
    (`app/mod.rs:503-523`, `dispatch.rs:490-496`).
    **Fix:** retry marker confirmation with backoff and/or surface a
    "stuck connecting — reconnect?" state on the divider. *(fixed in Phase
    4: `RemoteConnManager` re-arms `wait_for_client_marker` with bounded
    backoff, then surfaces a recoverable stuck state on the reconnect
    divider — `marker_retry_decision` is pure-tested)*

12. [x] **Summary generation has no timeout and no cancel.**
    `run_claude` blocks on `wait_with_output()` (`summary.rs:276`); a
    hung `claude` pins `SummaryState::Generating` forever and the
    Generate button is gated off while Generating (`dispatch.rs:562`).
    Also leaks a zombie on stdin-write failure (no kill/wait on the
    EPIPE path). **Fix:** timeout (kill + error state) + an Esc/cancel
    action; reuse the batched pane capture (see dup #D12) to cut ssh
    hops. *(fixed in Phase 4: 90s timeout + Esc cancel via the Worker
    harness; the child runs in its own process group and is
    killed+reaped — including subprocesses — on cancel/timeout/EPIPE. The
    D12 batched-capture dedup is still open.)*

### P2: races, leaks, papercuts

13. [ ] **Rename: no validation + order race.** Rename has no
    format/uniqueness checks (create has them, `dispatch.rs:17-38`);
    `tmux rename` errors are swallowed; dispatch patches
    `session_order` old→new immediately (`dispatch.rs:795-804`), so a
    failed rename (or a stale refresh snapshot racing it) makes
    `sync_order` drop the new name and re-append the old at the END —
    manual position lost. Fix: validate like create; drop or
    reconcile the eager order patch (docs/session-abstraction.md already
    recommended dropping it). *(`validate_unique_session_name` exists but
    only the create path calls it; rename still unvalidated and the eager
    order patch still runs at `dispatch.rs:780`)*
14. [x] **Hot-reload never applies `collapsed_sections`**
    (`dispatch.rs:962-980` copies everything else), and `save_config`
    then clobbers the hand-edited value. Dissolves in Phase 3.
    *(resolved by `apply_config`: collapse state is seeded once and
    deliberately not clobbered on reload; save writes the live value —
    state.rs:1309-1317)*
15. [x] **Settings page and theme picker can't scroll.** Settings renders
    ~36 rows into a clipping Paragraph (`ui/settings.rs:20-154`) — on a
    24-row terminal the last ~3 settings (incl. the newest one) are
    invisible but still selectable; the theme picker (25 themes,
    `ui/settings.rs:281-348`) never windows, selection walks off-screen.
    Exclude editor and pf list share the no-window pattern; the
    keybindings popup does it right (`ui/settings.rs:225-232`).
    *(fixed in Phase 3: both the settings page and theme picker now window
    around the selection via `scroll_window`; regression test
    `short_terminal_keeps_selected_setting_in_view`. Exclude editor / pf
    list still share the old pattern — minor, not addressed here.)*
16. [x] **Tab hit-rects are never clamped to the sidebar**
    (`ui/sidebar/header.rs:54-86`): at narrow sidebar widths the Agents
    rect extends into the PTY pane, and `mouse.rs:48` checks `tab_at`
    before the `in_sidebar` guard — clicks in the main pane's top row
    can switch tabs. *(fixed in Phase 1: `clamp_to_area` in `draw_header`
    + registry-wide `clamp_hits`; regression test
    `narrow_agents_tab_does_not_leak_into_pty`)*
17. [x] **Summary-card button hit-rects diverge from drawn position** below
    ~22-25 cols content width (`ui/sidebar/sessions.rs:240,268-285`) —
    clicking the card *title* can trigger Generate. *(fixed in Phase 1:
    the Generate/popup hit rects now derive from the drawn span offset,
    not a `width - w` right-edge assumption)*
18. [x] **`resize_pty` never resizes the upgrade pane** (`app/pty.rs:29-55`)
    — stale size until the upgrade exits. *(commit 8c58420)*
19. [x] **Disabling update-check can freeze the UI ~5s**: dropping
    `UpdateChecker` joins a worker that may be mid-HTTP
    (`infra/update.rs:71-78`); because of bug #8 that in-flight window
    exists at every startup. *(fixed in Phase 4: `UpdateChecker` rides the
    `Worker` harness, whose drop signals cancel and detaches — it never
    `join()`s on the UI thread)*
20. [x] **Host remove→re-add races.** `offboard_remote_host` doesn't clear
    `pending_remote_switch` / `remote_switch_verify`; a stale in-flight
    spawn can overwrite the fresh `RemoteConn` (last-write-wins,
    `app/mod.rs:466-481`) while the re-add's prelude `rm -f`s the
    marker glob — surviving connection can hold a dead marker.
    Fix shape: per-host spawn generation + full cleanup in offboard
    (Phase 4's `RemoteConnManager` is the home for this).
    *(fixed in Phase 4: `RemoteConnManager` gives each host a spawn
    generation stamped onto events — stale ones are dropped — and
    `offboard` clears the host's pending switch + switch-verify by
    construction)*
21. [x] **Switching sessions doesn't leave the Settings page** — the tmux
    client switches invisibly behind Settings (`dispatch.rs:365-386`
    never resets `main_view`). *(sidebar click now closes settings —
    commit bda58b7; main_view resets to Terminal on switch)*
22. [x] **SessionExecutor leaks one parked worker+sender per removed host**
    (`executor.rs:106-117`, never pruned); if `thread::spawn` fails the
    dead sender is cached and all future ops for that key vanish.
    *(fixed in Phase 5: `SessionExecutor::remove(host)` is called on
    offboard, and a sender is cached only after its worker thread spawns)*
23. [ ] **Remote refresh robustness:** a degraded host can hold the
    single-flight gate ~20s (4 sequential 5s ssh calls; comment claims
    5s, `infra/refresh.rs:236`); if the detached thread fails to spawn,
    `remote_in_flight` is never cleared (remote refresh permanently
    stuck); `mark_dead` freezes the sidebar silently in release builds.
24. [~] **`ps_snapshot` bypasses the timeout runner** (`agent.rs:291-297`,
    raw `Command::output` on the single refresh worker — a stuck `ps`
    freezes all refresh; `command.rs` exists to prevent exactly this).
    Same for `listeners.rs:78-105`. *(`ps_snapshot` now routes through
    `default_runner()` — agent.rs:300; `listeners.rs` still uses raw
    `Command::output` at lines 81/92)*
25. [ ] **Instance lock:** `/tmp/deck.lock` is machine-global (user B can't
    run deck while user A does); the stale-lock loop spins forever if
    `remove_file` keeps failing (sticky `/tmp`, other user's file);
    create-then-write TOCTOU lets two decks both start
    (`instance_guard.rs:51-64,112-124`). *(TOCTOU narrowed via
    `create_new(true)`, but path is still machine-global with no per-user
    scoping)*
26. [ ] **Keybinding shadowing outside the keybinding system:** digits 1-9
    are consumed before the binding lookup (and swallowed even when the
    jump is out of range); `f` (port-forward) is hardcoded *after* the
    lookup so a user binding on `f` silently breaks it, and `f` never
    appears in the keybindings viewer (`keyboard.rs:144-177`).
    *(digit handling and hardcoded `'f'` still present — keyboard.rs:151,167)*
27. [ ] **Bracketed paste bypasses the warning gate** (`app/mod.rs:714-719`)
    — typing is blocked while a warning popup is up, pasting isn't.
28. [x] **`eprintln!` while the alt screen is active** — keybinding
    warnings, reload warnings, upgrade-spawn errors are invisible
    (`app/mod.rs:217`, `dispatch.rs:946`, etc.). Needs an in-UI channel
    (the reload toast / warning popup already exist). *(commit 03f679e)*
29. [x] **`~/claude` is hardcoded** as the bootstrap session dir
    (`lifecycle.rs:24`) — author-specific; should be `$HOME` (tmux
    tolerates a missing `-c` dir, so this fails quietly). *(commit 8c58420)*
30. [ ] **First-connect zsh noise:** the attach prelude's `rm -f <glob>`
    prints `no matches found` into the PTY on zsh hosts (redirection
    can't suppress it; use `rm -f -- <glob> 2>/dev/null || true` with
    the glob expanded by `sh -c`, or guard with `setopt null_glob`-safe
    form / `find -delete`). *(prelude now appends `2>/dev/null` but the
    glob is still bare, so zsh's own nomatch error isn't suppressed —
    remote_spawn.rs:143)*
31. [~] Minor UI: reload bar can render a double ellipsis (`……`,
    `ui/reload.rs:88-93`); the summary-language editor's "Enter save /
    Esc cancel" hint is clipped at every size (`ui/settings.rs:359,393`);
    `wrap_markdown` drops leading spaces so nested lists in summaries
    flatten (`ui/text.rs:144`); `truncate(s, 0)` returns width-1 `.`
    (latent); stale help text "Left/right cycles … language" for what is
    now a free-text editor (`ui/settings.rs:95`). *(reload bar was
    rewritten in the cleanup batch; the rest are unverified/likely open)*
32. [ ] Minor behavior: `NumberKeyJump` can land on a row hidden in a
    collapsed group (j/k skip them); add-remote can't add host `web`
    when `web-prod` exists (substring match always wins,
    `add_remote.rs:55-65`); OSC52 is forwarded from the local pane even
    when a Plugin/Settings view hides it (contradicts the comment);
    theme apply + local agent focus run synchronous tmux calls on the
    render thread (bounded 1s each); `Pty::write` is a blocking
    `write_all` on the UI thread.

---

## Part 2 — Redundancy worth deleting

- [x] **D1. Point-in-rect ×7**: `state.rs:1361,1370,1841-1868` (5 copies),
  `mouse.rs:8`, inline `mouse.rs:178`. ratatui has `Rect::contains`.
  Dissolves into Phase 1's hit registry. *(done: the hand-rolled rect
  tests collapsed into `HitRegions::hit` using `Rect::contains`)*
- [x] **D2. Config↔state mapping ×5**: `Config` struct, `AppState::new`'s
  15 params, post-seed in `App::new` (`mod.rs:256`), `reload_config`
  (`dispatch.rs:962`), `save_config` (`app/update.rs:13`). Already
  produced bug #14. Dissolves in Phase 3. *(done: the 17 persisted prefs
  live in a `Prefs` unit; `Prefs::from_config`/`to_config` are the only
  two mapping sites — `apply_config` and `save_config` call them)*
- [x] **D3. Settings rows ×3**: entries vec (`ui/settings.rs:36-102`),
  positional match `0..=9` (`reduce.rs:488`), `SETTINGS_ITEM_COUNT`
  (`state.rs:135`). Adding the 10th setting (working tree) touched ~12
  sites across 5 layers. Dissolves in Phase 3. *(done: one
  `SETTING_ROWS` descriptor table in `app/settings.rs` drives renderer +
  reducer; `SETTINGS_ITEM_COUNT` and the positional match are gone)*
- [ ] **D4. Picker overlays ×3**: `NewSessionState` / `AddRemoteState` /
  `ExcludeEditorState` share input+items+filtered+selected+error and a
  duplicated refilter-clamp; their draw fns are line-for-line the same
  shape (`ui/new_session.rs` vs `ui/add_remote.rs`). One generic
  filter-picker (model + widget). Phase 6.
- [ ] **D5. Bounded/wrapping cursor steps ×9+**: NewSessionPrev/Next,
  AddRemotePrev/Next, ExcludeEditorNext/Prev, PfFocusUp/Down,
  SettingsNext/Prev, ThemePickerNext/Prev, `cycle_frame_rate_limit`,
  `cycle_agents_probe_interval`, `cycle_field`/`set_mode`. Two helpers
  (`step_clamped`, `step_wrapped`) cover all. *(done: clamped steps share
  `step_clamped`; the wrapping cyclers already shared `cycle_option`, so
  no separate `step_wrapped` was needed)*
- [x] **D6. Identical menu constants**: `SESSION_MENU_ITEMS` ==
  `REMOTE_SESSION_MENU_ITEMS` == `PLACEHOLDER_DISABLED_ITEMS`
  (`state.rs:31-39`), and `session_menu_items()` branches local/remote
  to return the same list. Collapse; becomes an enum in Phase 2.
  *(done: `REMOTE_SESSION_MENU_ITEMS` removed earlier, and Phase 2
  converted the lists to a `MenuItem` enum keyed by variant)*
- [x] **D7. Dead-host cleanup vs `offboard_remote_host`** duplicate the
  detach choreography (`app/mod.rs:551-584` vs `dispatch.rs:443-463`);
  one `detach_host_view(host)` — also the natural home for the missing
  `pending_remote_switch` cleanup (bug #20). *(done in Phase 4: one
  `detach_host_view`, called by both the dead-host reap and offboard)*
- [ ] **D8. `RemoteConn` placeholder literal ×3** (`mod.rs:277,493`,
  `dispatch.rs:405`) → `RemoteConn::connecting()/failed()`. *(an
  `is_live` helper was added, but no `connecting()`/`failed()` constructors)*
- [ ] **D9. Warning-aware focus postlude ×4** in dispatch arms
  (`dispatch.rs:92,107,123,138`) → one helper.
- [ ] **D10. Per-host attachable-session-names ×3** (`dispatch.rs:875,1101,
  1173`) → `AppState::host_session_names(host)`.
- [ ] **D11. Local/remote agent status classification ×2**
  (`refresh.rs:222` vs `remote_tmux.rs:166`) — same logic, different
  capture; unify behind one classify-all fed by a capture closure.
- [ ] **D12. Summary per-pane capture re-implements the batched
  `capture_panes_with`** (`summary.rs:111` vs `remote_tmux.rs:199`) —
  K agents on a dead host = 5K-second stall. Reuse the batch. *(tied to
  bug #12, still per-pane)*
- [ ] **D13. `persist_session_order` ×2** (`tmux.rs:77` vs
  `remote_tmux.rs:547`) — the `-t '=name'` fix (bug #3) currently has
  to be made twice; share the arg builder. *(still two separate
  functions, tmux.rs:92 / remote_tmux.rs:523)*
- [~] **D14. ssh-error classification ×2**, **dir-error mapping ×2**,
  **`default_runner()` ×2** (remote_tmux/local/tmux) — extract one each.
  *(`default_runner` now single — command.rs:35; ssh-error / dir-error
  mapping not verified-extracted)*
- [~] **D15. UI duplicates**: `scrollbar_cells` verbatim ×2
  (`sessions.rs:510` / `summary_popup.rs:84`); markdown-window painting
  loop ×2 (+ error arm); reload rewrap block ×2; selected-row styling ×3
  beside `list_item_line`; rename + summary-language editors hand-roll
  the `form::field_row` stanza; mouse PTY offset math ×2
  (`mouse.rs:267,279`); `ExcludeEditorConfirm` bodies ×2
  (`reduce.rs:648,662`); checker spawn+request ×2 (`app/update.rs:52`
  vs `136`). *(`ui/widgets.rs` now centralizes `scrollbar_cells`,
  `list_item_line`, `scroll_window`, `popup_frame`, `centered_rect`,
  `style_textarea`; remaining dups — field_row reuse, PTY offset math,
  checker spawn — open)*
- [~] **D16. Seven hand-rolled worker patterns** (refresh, update checker,
  port-forward, executor, remote spawner, focus one-shots, summary
  one-shot) — each with different lifecycle/timeout/drop semantics;
  bugs #12/#19/#23/#24 are all instances of the missing policy. Phase 4.
  *(Phase 4 added the shared `Worker<Req,Res>` harness (`infra/worker.rs`)
  and migrated the two workers where the bugs lived — update checker (#19)
  and summary (#12). The keyed-FIFO executor, single-flight refresh, and
  long-running port-forward task keep bespoke shapes the generic harness
  doesn't model; focus one-shots (N workers → one shared drain) stay
  ad-hoc. #23/#24 remain open.)*
- [~] **D17. Per-frame waste**: `render.rs:36-57` defensively clones
  context-menu/new-session/add-remote/pf/warning/summary text each
  frame; `agent_rows()` clones every `DetectedAgent` per call and is
  called per frame *and* per keystroke (`focusable_count`); `current_layout`
  is rebuilt per mouse event. Cache/borrow once the model split (Phase 5)
  makes ownership clear. *(Phase 5: `AgentRow` now borrows instead of
  cloning, and `focusable_count` uses a cheap `agent_count` length path;
  the render loop already computes layout/agent_rows once per frame. The
  defensive overlay-text clones in `render.rs` are left as-is.)*
- [x] **D18. `AppState.filtered` is vestigial**: exclusion happens in the
  refresh worker (`infra/refresh.rs:187`); `recompute_filter` is the
  identity permutation (`state.rs:2162`). All `sessions[filtered[i]]`
  double-indexing is dead complexity — and the kill guard checks
  `sessions.len()` while `switch_to` picks from `filtered`, safe only
  while they coincide. Delete it (Phase 5). *(done in Phase 5: `filtered`
  removed, entries indexed directly, kill-guard asymmetry fixed)*

---

## Part 3 — The plan

Principles: each phase is independently shippable and behavior-preserving
(except Phase 0, which is bug fixes); bugs whose real fix is structural
are fixed *by* their phase, not patched twice; the local/remote rule
(one type, `Option<String>` host key) stays the north star.

### Phase 0 — Correctness triage (small, independent fixes) — [x] DONE

Order: #1 config wipe, #2 force-kill match, #3 `-t '='` exact targeting
(via the shared builder from D13), #6 watcher self-reload, #8 dead update
cache, #5 footer constant (stopgap: change `3` to `2` + regression test;
real fix in Phase 1), #10 kill pre-switch condition, #9 kill policy
unification, #4 summary log location/perms, #18 upgrade-pane resize,
#29 `$HOME` bootstrap dir, #28 route startup warnings into the UI.
Each lands with a test where the layer permits.
*(All Phase 0 items landed across commits 95816d2 … 8c58420. Note #3 did
NOT extract the shared D13 builder — the `-t '='` fix was made in both
`tmux.rs` and `remote_tmux.rs` separately.)*

### Phase 1 — One geometry, one hit-test (fixes the #5/#16/#17 class) — [x] DONE

Problem: renderer and hit-tester compute sidebar geometry independently
(`2+` vs `3+`), and per-frame hit rects are scattered across 10
`AppState` fields filled from `draw_sidebar`'s 7-tuple return.

Plan:
- [x] Move every shared formula (footer rows, header rows, card heights)
  into `ui/layout.rs` next to `plugin_block_rows` — single source for
  renderer and `session_row_hit`. *(`sidebar_footer_height`, `card_height`,
  `tab_col_ranges`, etc. now live in `ui/layout.rs`)*
- [x] Introduce one `HitRegions` struct (registry pattern): named slots
  (tabs, banner, menu button, summary button/popup/card, kill yes/no)
  plus vecs (divider hits, agent hits). `draw_sidebar` returns it whole;
  `AppState` stores it as one field; rects are clamped to their drawing
  area at capture time (fixes #16). *(`HitRegions` + `HitKind` now live in
  `model/state.rs`; `AppState` stores one `hit_regions` field; rects
  clamped in `header.rs`/`sessions.rs` and re-clamped at capture via
  `clamp_hits`)*
- [x] One resolver `HitRegions::hit(col,row) -> Option<HitKind>` consulted
  by `mouse.rs` — replaces the seven hand-rolled rect tests (D1) with
  `Rect::contains`, and makes hit-test priority explicit in one match.
  *(wheel-scroll routing intentionally reads `summary.card` directly, not
  via the priority resolver — see the regression test)*
- [x] Test: a TestBackend round-trip asserting renderer allocation ==
  `sidebar_footer_height` and that every captured rect ⊆ sidebar area.
  *(plus a `mouse_to_action` regression test for the wheel-over-card path)*

Result: a new clickable element = one slot + one HitKind arm; geometry
drift becomes a compile-time/test-time error, not a click-dead row.

### Phase 2 — Input modality and typed actions (fixes the #7 class) — [x] DONE

Problem: each modal overlay needs a hand-written early-return in both
`keyboard.rs` and `mouse.rs`; five overlays drifted mouse-side. Menus
dispatch on `&'static str` labels; `Action` is a flat ~100-variant enum.

Plan:
- [x] `AppState::active_modal() -> Option<Modal>` — one ordered enum of
  every overlay modal. Both key and mouse mappers resolve the modal
  *first* and route events to it; only `None` falls through to
  sidebar/PTY handling. *(the update-warning popup stays a separate
  selective gate — `warning_blocks_action` blocks only focus/forward
  actions, not a swallow-everything modal, so folding it in would change
  behavior. Left as-is.)*
- [x] Replace string menu items with a `MenuItem` enum carrying `label()`;
  `MenuKind` holds `&[MenuItem]` (kills D6; a renamed label can no
  longer silently disable an action).
- [x] Namespace `Action` into sub-enums (`Pf(..)`, `Settings(..)`,
  `NewSession(..)`, `Summary(..)`, `Menu(..)`, `AddRemote(..)`); the
  reducer match delegates to per-domain functions (`reduce_pf`,
  `reduce_settings`, …). Mechanical, big readability win.
- [x] Test: a table-driven parity test — for every modal, key and mouse
  mappers must both refuse to produce session-switching actions
  (`tests/unit/app/action/modality.rs`).

### Phase 3 — Settings as data (fixes the #14/#15 class, kills D2/D3) — [x] DONE

Problem: a new setting touches ~12 sites in 5 layers; config↔state
mapping is enumerated five times; the settings page can't scroll.

Plan:
- [x] Split `Config` into the persisted-preferences subset (`Prefs`) and
  runtime-only data; `AppState` holds `prefs: Prefs` *as a unit* instead
  of ~15 exploded fields. `Prefs::from_config`/`to_config` are the only
  mapping sites (App::new, reload, save all route through them). Bug #14
  stays impossible (`collapsed_sections` is kept out of `Prefs` and
  seeded once). *(round-trip identity test in `tests/unit/model/state.rs`)*
- [x] A static descriptor table drives the settings page:
  `SettingRow { label, help, value: fn(&AppState)->String,
  adjust: fn(direction)->Action }`. Renderer iterates it; reducer
  indexes it; `SETTINGS_ITEM_COUNT` and the `0..=9` match are deleted.
  Stale help text can't survive because help lives beside behavior.
  *(`SETTING_ROWS` in `app/settings.rs`; `help` is `fn(&AppState)->String`
  since some help is value-dependent)*
- [x] Give the settings page and theme picker the same `scroll_window`
  treatment the keybindings popup already has (fixes #15).
- [x] Generalize `step_clamped` and replace the clamped cursor loops (D5).
  *(the wrapping cyclers already shared `cycle_option`)*

### Phase 4 — Decompose `App` (old plan's Phase 4; fixes #19/#20/#22 class) — [x] DONE

> Done except #22 (executor sender prune — the executor was left bespoke;
> that prune is Phase 5) and the D16 long-tail (refresh/executor/
> port-forward workers kept bespoke). The four steps' structural goals and
> bugs #11/#12/#19/#20 + D7 all landed.

Problem: `App` has ~25 fields mixing six orchestration concerns;
`dispatch.rs` is 1,440 lines; the remote-connection state machine
(markers, pending switches, verify) is spread across `mod.rs` event
drains, `dispatch.rs` gating, and `remote_spawn.rs`, with invariants
living in comments. Seven worker patterns each hand-roll lifecycle.

Plan, in order:
1. [x] **`RemoteConnManager`** owning `remote_conns`, `remote_spawner`,
   `pending_remote_switch`, `remote_switch_verify`, `active_remote`,
   marker gating, onboard/offboard/respawn, and the shared
   `detach_host_view` (D7). Give spawns a per-host generation id so
   stale events can't clobber fresh connections (bug #20), and make
   offboard clear pending state by construction. Marker retry (bug #11)
   lands here. Most of its logic becomes pure functions over the conn
   map → unit-testable without ssh.
2. [~] **`Worker<Req, Res>` harness** (one struct: spawn, `try_recv` drain,
   drop policy, optional timeout) replacing the seven hand-rolled
   thread+channel pairs (D16). Summary gets timeout+cancel (bug #12);
   update checker stops joining on the UI thread (bug #19); one-shot
   focus threads stop being ad-hoc. *(harness landed; summary (#12) +
   update checker (#19) migrated. Executor/refresh/port-forward kept
   bespoke — semantics the generic harness doesn't model; focus one-shots
   stayed ad-hoc. Noted for a later pass.)*
3. [x] **Run-loop pumps**: extract each drain block of `run()` into
   `pump_local_pty() -> Redraw`, `pump_remote_events() -> Redraw`, …
   where `Redraw` is `No | Soft | Force` and merges; the loop body
   becomes ~20 readable lines and the `needs_render`/`force_render`
   bookkeeping exists once. A tiny `Ticker` struct covers the four
   periodic timers (refresh, config poll, update, blink/spinner).
4. [x] Split `dispatch.rs` along its existing seams: `remote_conn.rs`
   (moves into the manager), `new_session_flow.rs`, `reload.rs`.

### Phase 5 — Split the model; unify the session list (kills D17/D18) — [x] DONE

Problem: `state.rs` is 2,234 lines holding ~10 concerns; `AppState` has
~45 pub fields; `Effect` (app vocabulary) lives in the model; local and
remote sessions are two types + two stores stitched together by flat-
index arithmetic in six places; `filtered` is vestigial; model imports
`ui::layout` (layering inversion).

Plan, in order:
1. [x] Mechanical file split, no behavior change: `model/effects.rs`
   (Effect, SideEffect, request DTOs + test macros), `model/menu.rs`,
   `model/forwards.rs`, `model/summary.rs` (one `SummaryCard` struct
   absorbing the loose summary fields), `model/overlay.rs`,
   `model/geometry.rs` (sidebar layout building + hit decode; the
   shared constants move *down* here so `ui` depends on model, never
   the reverse). `state.rs` keeps `AppState` + focus/filter/order.
   *(SummaryCard landed in 5.2; the rest in 5.1; model no longer imports ui)*
2. [x] Delete `filtered` (D18); replace `sessions[filtered[i]]` with direct
   indexing and fix the kill-guard asymmetry while there.
3. [x] Unify the stores: one `Vec<SessionEntry>` where
   `SessionEntry { host: Option<String>, name, dir, kind }` and
   `kind ∈ {Live{is_current, idle}, Connecting, Unreachable, NoSessions}`
   replaces `SessionRow` + `RemoteSessionRow` + reserved-name sentinels
   (`"(no sessions)"` as a magic string disappears). `session_target`'s
   decode, `focusable_index_for`, `section_key_of_focus`, and the
   reducer's `idx - local_count` arithmetic all collapse to "look at
   the entry". This is the repo's own stated rule applied to its oldest
   exception, and the single biggest simplification available.
   *(done + dedicated review; behavior-preserving for local & remote)*
4. [~] Newtype the host key (`HostKey` wrapping `Option<Arc<str>>` or an
   interned id) used by `agents`, `collapsed_sections`, `remote_conns`,
   executor senders, `ForwardKey` — kills per-lookup `Option<String>`
   clones; then cache `agent_rows`/layout per frame (D17). *(HostKey
   landed for `agents`/`collapsed_sections`/executor senders with an
   allocation-free `Borrow` lookup; `remote_conns`/`ForwardKey` and the
   Effect/dispatch DTOs deliberately stay `Option<String>` — converting
   them is churn at unrelated layers. D17 borrow-not-clone done.)*
5. [x] Prune the executor's sender map on offboard (bug #22).

### Phase 6 — UI consolidation — [~] PARTIAL

- [ ] One generic **filter-picker widget** (+ shared picker state) for
  new-session / add-remote / exclude-editor (D4).
- [~] Move `scrollbar_cells`, the markdown-window painter, and a padded-
  selectable-row helper into `ui/widgets.rs` (D15); use
  `form::field_row` for the rename and summary-language editors.
  *(`ui/widgets.rs` now holds `scrollbar_cells` + `list_item_line` +
  others; markdown painter and field_row reuse still pending)*
- [ ] **Theme semantics**: add `error`/`warning`/`success` slots (today
  `pink` simultaneously means working-status, error, down-health,
  unreachable, *and* a decorative host accent — a pink-accented host
  shows "healthy" in the unreachable color). Map old themes mechanically.
- [ ] `ui/text.rs`: fix `truncate(_, 0)`, preserve leading spaces in
  `wrap_markdown`, and sweep the byte-`len()` width computations
  (settings/theme-picker/menu/tabs) to `UnicodeWidthStr` — summaries
  and session names are CJK-bearing.
- [ ] Port-forward overlay: take `&mut Frame` like every other overlay.

### Phase 7 — Tests that hold the line — [ ] NOT STARTED

Woven through the phases, but tracked: geometry-equivalence TestBackend
test (Phase 1); key/mouse modality parity (Phase 2); prefs round-trip
`apply_config(to_config(s)) == s` (Phase 3); pure-function tests for
marker gating / spawn generations / reload diff (Phase 4); remote
command-construction snapshots — attach prelude, persist-order, focus
script — plus `pty.rs` key/mouse encoders and executor FIFO ordering
(today: `dispatch.rs` 1,440 lines and `remote_tmux.rs` 1,084 lines have
zero direct tests); `truncate`/`wrap_markdown` CJK edges (Phase 6).

### Docs cleanup (cheap, do alongside Phase 0) — [x] DONE

- [x] `CLAUDE.md`: config path is `~/.config/deck/config.yaml` (not
  `.json`); the architecture section references `docs/ARCHITECTURE.md`
  (doesn't exist), `app.rs`/`state.rs`/`ui.rs`/`git.rs` flat files
  (now directories; no `git.rs`). Either write a small ARCHITECTURE.md
  reflecting the real `model/app/infra/ui/session` split or point at
  this file. *(CLAUDE.md updated to the real module split, commit 7669e54)*
- [x] Stale comments flagged in review: `tmux.rs:24-28` and
  `tmux_parse.rs:31-35` contradict `SESSION_LIST_FORMAT_SSH` (remote
  *does* request `@deck_order`); `dispatch.rs:927` says config.json;
  `local.rs:72` references a moved function; `state.rs:1485`'s
  "mirroring" comment (bug #5).
- [x] Delete `docs/high-roi-cleanup.md` (superseded by this file).
  *(removed in commit 7669e54)*
