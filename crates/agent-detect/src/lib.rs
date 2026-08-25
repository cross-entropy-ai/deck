//! Detect interactive coding agents (Claude Code, Codex) from pane process
//! trees, and classify their visible terminal-buffer status.
//!
//! This crate is the pure, IO-free core of deck's agent detection: given a
//! list of tmux panes ([`PaneInfo`]) and a process-table dump (`ps -axo
//! pid=,ppid=,args=`), [`detect_agents`] walks each pane's subtree and returns
//! the interactive agent (if any) running in it. [`classify_verdict`] reads an
//! agent's raw pane buffer (and, when available, its OSC pane title) and
//! derives a [`Verdict`] around a traffic-light [`AgentStatus`];
//! [`classify_status`] is the status-only convenience form. The rule set is
//! measured against real captures and borrows shapes from herdr's published
//! detection manifests — see deck's `docs/agent-status-plan.md`.
//!
//! Detection targets *interactive* invocations only: `pane_current_command`
//! is unreliable (it shows Claude Code's version string and flips while a
//! subprocess runs), so the whole pane subtree is searched and the agent's
//! `argv` is matched. Headless / non-interactive forms are excluded. See
//! deck's `docs/agent-integration.md` for signature research.
//!
//! The crate depends only on `regex` and carries no tmux, ssh, or deck-specific
//! state; runtime collection (running `ps`, capturing panes, timeouts) lives in
//! deck.

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Codex,
}

/// One tmux pane's identity, fed to `detect_agents`. `session`/`window` are
/// display fields (`window` is the window *name*); `pane_id` is the stable
/// `%N` switch handle — names and indices both churn as panes/windows change,
/// so only `pane_id` is a safe target.
#[derive(Debug, Clone)]
pub struct PaneInfo {
    pub pid: u32,
    pub session: String,
    pub window: String,
    pub pane_id: String,
    /// OSC pane title (`#{pane_title}`); empty when unset. Feeds the title
    /// tier of [`classify_verdict`].
    pub title: String,
    /// `#{window_activity}` — epoch seconds of the window's last output,
    /// on the *server's* clock. `None` when the server doesn't know the
    /// format variable. Freshness against the same server's "now" is the
    /// level-triggered working signal (see `docs/agent-status-plan.md`).
    pub window_activity: Option<u64>,
    /// `#{window_panes}`. The activity clock is window-scoped, so it is
    /// only trustworthy for an agent that has its window to itself.
    pub window_panes: Option<u32>,
}

/// An interactive agent located in a specific pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedAgent {
    pub kind: AgentKind,
    pub session: String,
    pub window: String,
    /// Stable `%N` pane id — the switch/focus target.
    pub pane_id: String,
    /// Traffic-light health from the pane buffer (see [`classify_status`]).
    /// `detect_agents` has no buffer, so it leaves this `Unknown` for the
    /// gathering layer to fill in.
    pub status: AgentStatus,
    /// The classifier saw an agent-owned viewer overlay (transcript, model
    /// picker) that can't show live state. The gathering layer has no
    /// memory, so the stateful side (deck's snapshot apply) resolves this
    /// by keeping the pane's previous status; `status` is `Unknown` here.
    pub keep_previous: bool,
}

/// Traffic-light health shown as a colored dot per agent row: green =
/// working, red = idle, yellow = waiting for input, default = unknown (not
/// captured, or no classifier yet). The renderer
/// (`ui::sidebar::sessions::recolor_agent_dot`) maps color off this status,
/// not the glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentStatus {
    Working,
    Idle,
    Waiting,
    #[default]
    Unknown,
}

/// A screen classification with the confidence metadata the merge layer
/// needs (modeled on herdr's `AgentDetection`). The bare [`AgentStatus`]
/// can't express two things the screen knows: "this screen can't show live
/// state at all" and "there is visibly a dialog / a finished turn here".
/// Hooks and the activity clock may *light* Working; only positive screen
/// evidence may *retract* it — these flags carry that evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    pub status: AgentStatus,
    /// The visible screen is an agent-owned viewer (transcript overlay,
    /// model picker) showing history instead of live state; the caller
    /// should keep the previous status. `status` is `Unknown` here.
    pub keep_previous: bool,
    /// The screen visibly shows a dialog awaiting the user's choice —
    /// the strongest tell there is; may override a non-blocked hook state.
    pub visible_blocker: bool,
    /// The screen shows positive completed/interrupted evidence (not just
    /// "nothing matched"); strong enough to retract a stale hook Working.
    pub visible_idle: bool,
}

impl Verdict {
    fn status(status: AgentStatus) -> Self {
        Verdict {
            status,
            keep_previous: false,
            visible_blocker: false,
            visible_idle: false,
        }
    }

    fn blocker() -> Self {
        Verdict {
            visible_blocker: true,
            ..Verdict::status(AgentStatus::Waiting)
        }
    }

    fn idle_tell() -> Self {
        Verdict {
            visible_idle: true,
            ..Verdict::status(AgentStatus::Idle)
        }
    }

    fn keep_previous() -> Self {
        Verdict {
            keep_previous: true,
            ..Verdict::status(AgentStatus::Unknown)
        }
    }
}

impl DetectedAgent {
    /// Compact `session:window` location for display. The pane index is
    /// omitted — it's noise in the sidebar; the real tmux target is `pane_id`.
    pub fn location(&self) -> String {
        format!("{}:{}", self.session, self.window)
    }
}

/// Status-only convenience form of [`classify_verdict`], with no pane title.
/// Existing call sites keep this shape; new callers that have the title and
/// care about the verdict flags use [`classify_verdict`] directly.
pub fn classify_status(kind: AgentKind, buffer: &str) -> AgentStatus {
    classify_verdict(kind, buffer, None).status
}

/// Classify a `kind`'s state from its raw pane buffer plus, when the caller
/// has it, the pane's OSC title (`#{pane_title}`). Pure (no IO) so it runs
/// cheaply every refresh and is trivial to unit-test.
///
/// The title is a tell of its own where the agent drives a spinner through
/// it while a turn runs (older Claude versions, Codex per herdr's manifest)
/// or flags "Action Required" — those survive the streaming blind spot,
/// where the buffer shows no tell at all. Measured on Claude 2.1.241 the
/// title stays "✳ <summary>" throughout, so for it the title tier is inert:
/// neither working nor idle evidence.
pub fn classify_verdict(kind: AgentKind, buffer: &str, title: Option<&str>) -> Verdict {
    // An unset title comes through as an empty string; treat it as absent.
    let title = title.map(str::trim).filter(|t| !t.is_empty());
    match kind {
        AgentKind::Claude => claude_verdict(buffer, title),
        AgentKind::Codex => codex_verdict(buffer, title),
    }
}

/// How stale (seconds, on the tmux server's own clock) `#{window_activity}`
/// may be and still count as "output is flowing". Two refresh ticks: tight
/// enough that a finished turn goes dark quickly, loose enough that one
/// probe's jitter doesn't flap the dot.
pub const ACTIVITY_FRESH_WINDOW_SECS: u64 = 2;

/// Whether the pane's window emitted output within the freshness window.
/// `None` — no signal — unless every ingredient is present AND the agent has
/// the window to itself: `#{window_activity}` is window-scoped, so in a
/// split window a neighbor's output would masquerade as agent activity.
/// `now` must come from the same machine's clock as `window_activity`
/// (locally `SystemTime::now`, remotely the probe's `date +%s`), or skew
/// becomes part of the window.
pub fn activity_fresh(
    window_activity: Option<u64>,
    window_panes: Option<u32>,
    now: Option<u64>,
) -> Option<bool> {
    if window_panes != Some(1) {
        return None;
    }
    let (activity, now) = (window_activity?, now?);
    Some(now.saturating_sub(activity) <= ACTIVITY_FRESH_WINDOW_SECS)
}

/// Merge the screen verdict with the activity clock into the status the
/// sidebar shows. The ordering encodes one asymmetry, measured in
/// `docs/agent-status-plan.md`: *lighting* Working tolerates error, but only
/// **live** evidence may keep it lit, and only **positive** evidence may
/// retract a source that says Working.
///
/// - A visible dialog outranks everything: if a blocker is on screen, the
///   user is being waited on, whatever any clock says.
/// - A viewer overlay yields `Unknown` + the caller-resolved
///   [`DetectedAgent::keep_previous`] (this pure layer has no memory).
/// - A live working tell or fresh output wins over the transcript's idle
///   remnants: a completed-turn line from the *previous* turn stays on
///   screen while the next turn streams, and fresh output disproves it.
/// - The positive idle tells (completed / interrupted lines) come next —
///   they exist to retract stale *hook* Working (a coming source), which the
///   weak fallback must never do.
pub fn merge_status(screen: Verdict, activity_fresh: Option<bool>) -> AgentStatus {
    if screen.visible_blocker {
        return AgentStatus::Waiting;
    }
    if screen.keep_previous {
        return AgentStatus::Unknown;
    }
    if screen.status == AgentStatus::Working || activity_fresh == Some(true) {
        return AgentStatus::Working;
    }
    if screen.visible_idle {
        return AgentStatus::Idle;
    }
    // Weak readings pass through: a dialog-ish text match stays Waiting, an
    // unrecognized screen stays the weak Idle, an empty capture Unknown.
    screen.status
}

/// How many lines up from the bottom to consider at all — the capture is
/// roughly one screen, but cap it so a busy transcript can't sway the verdict.
const MAX_SCAN_LINES: usize = 40;

/// A live spinner/tool line sits a few lines above the input box. The
/// bare-spinner/bare-tool tells (which could match completed transcript) are
/// gated to this many bottom **non-blank** lines, so blank rows around the
/// input box don't push the spinner out of the window.
const LIVE_TAIL_LINES: usize = 12;

/// The phrase Claude Code prints only while a turn is interruptible:
/// "… · esc to interrupt)". Most reliable "working" tell (verb-, glyph-,
/// description-independent) and gone from the transcript once the turn ends,
/// so it can match anywhere in the scanned tail without hitting stale lines.
const CLAUDE_INTERRUPT_HINT: &str = "esc to interrupt";

/// Spinner progress tail with a live timer: "… (5m 21s", "… (30s)". Backup
/// tell for spinner lines whose interrupt hint is truncated/wrapped off.
/// Requiring a *duration* right after "(" rejects completed tool lines
/// ("Reading 1 file… (ctrl+o to expand)") and stray prose ("… (see above)").
fn working_timer_tail() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"(?:\u{2026}|\.\.\.)\s*\(\s*\d+\s*[hms]").unwrap())
}

/// A line led by a Claude Code animated spinner glyph (rotating sparkle/star
/// frames; NOT "* · •", which double as markdown bullets — left to the gated
/// gerund/timer tells). The glyph shows while a turn runs ("✻ Waiting for 1
/// dynamic workflow to finish"), so the leading glyph alone signals work.
/// Caller gates it to the bottom [`LIVE_TAIL_LINES`] non-blank lines and runs
/// it only after the completed-turn check, since the finished summary
/// ("✶ Cogitated for 8s") reuses the same glyph.
fn working_spinner_glyph() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| {
        regex::Regex::new(
            r"^\s*[\u{2722}\u{2726}\u{2727}\u{2731}\u{2733}\u{2734}\u{2735}\u{2736}\u{2737}\u{2738}\u{2739}\u{273a}\u{273b}\u{273d}\u{2749}\u{274b}]",
        )
        .unwrap()
    })
}

/// A bare thinking spinner whose glyph isn't in [`working_spinner_glyph`]'s
/// set, keyed on structure: leading symbol + gerund "…ing" before the trailing
/// ellipsis ("⟳ Frobnicating…"). Caller-gated to the bottom [`LIVE_TAIL_LINES`].
fn working_spinner_tail() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"(?i)^\s*[^\w\s].*ing(?:\u{2026}|\.\.\.)\s*$").unwrap())
}

/// A tool call in flight with no parenthetical ("Reading 1 file…", "Running
/// command…"). Tool verbs are a stable finite set, so a whitelist is safe and
/// avoids matching stray prose. Requires the live trailing ellipsis a completed
/// tool line lacks; caller gates it to the bottom [`LIVE_TAIL_LINES`].
fn working_tool_tail() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| {
        regex::Regex::new(
            r"(?i)^\s*(?:reading|writing|editing|updating|running|executing|searching|grepping|fetching|building|testing|installing|committing|pushing|analyzing|checking)\b.*(?:\u{2026}|\.\.\.)\s*$",
        )
        .unwrap()
    })
}

/// Dialog *chrome*: strings only Claude Code's own selection UI paints —
/// the `❯ 1.` selector and the Bash-permission footer. These are strong
/// evidence (`visible_blocker`): a dialog is on screen right now, whatever
/// any other source says. Dialogs are bottom-anchored and clip from the
/// top, so their chrome is visible whenever the dialog is.
const CLAUDE_BLOCKER_CHROME: &[&str] = &["\u{276f} 1.", "tab to amend"];

/// Dialog *question text*. Weak evidence on purpose: Claude routinely ends
/// a finished turn with prose like "Do you want me to continue?", and that
/// line lingers in the transcript. At rest it still reads Waiting (the
/// agent did ask the user something), but it must not outrank live
/// sources — a fresh activity clock or hook report says the next turn is
/// already running past it.
const CLAUDE_WAITING_PROSE: &[&str] = &[
    "do you want",
    "waiting for permission",
    "run a dynamic workflow?",
];

/// The line Claude Code prints in the transcript when the user interrupts a
/// turn (Esc): "⎿  Interrupted · What should Claude do instead?". A positive
/// idle tell: interruption fires no lifecycle hook at all, so this line is
/// what retracts a stale hook-reported Working.
const CLAUDE_INTERRUPTED: &str = "interrupted \u{b7} what should claude do instead";

/// Claude Code's busy-spinner pane title: a braille frame (≤ 2.1.227) or a
/// half-circle frame (2.1.228+) leading the OSC title, per herdr's claude
/// manifest. Measured on 2.1.241 the title does NOT spin — it stays
/// "✳ <summary>" straight through a running turn — so this tier only ever
/// fires on the older versions herdr documented, where a spinning title is
/// unambiguous and outranks the buffer. Crucially the reverse rule is dead
/// on 2.1.241: "✳ " appears mid-turn, so it must never count as idle
/// evidence (herdr's `osc_title_idle` is wrong for tmux consumers).
fn claude_title_working() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"^[\u{2800}-\u{28FF}\u{25D0}-\u{25D3}] ").unwrap())
}

/// A finished-turn summary line: "…ed for <number>…" (the past-tense verb
/// Claude Code prints when a turn completes, e.g. "Cogitated for 8s").
fn completed_line(lower: &str) -> bool {
    lower.match_indices("ed for ").any(|(i, m)| {
        lower[i + m.len()..]
            .trim_start()
            .starts_with(|c: char| c.is_ascii_digit())
    })
}

/// Classify Claude Code's pane. The title tier goes first (herdr ranks the
/// spinning title above every buffer rule — it is live for the whole turn,
/// while buffer tells vanish during plain-text streaming); then the buffer's
/// bottom slice is scanned bottom-up; the lowest status-bearing line wins
/// (lines above are stale transcript). Per line:
/// - in-flight turn → `Working`, via verb-independent tells that survive
///   Claude's rotating spinner verbs ([`CLAUDE_INTERRUPT_HINT`],
///   [`working_timer_tail`], [`working_spinner_glyph`],
///   [`working_spinner_tail`], [`working_tool_tail`]);
/// - dialog chrome → `Waiting` + `visible_blocker`; bare question prose →
///   weak `Waiting` (see [`CLAUDE_WAITING_PROSE`]);
/// - finished-turn summary "…ed for <number>…" or the interrupt line
///   ("Interrupted · What should Claude do instead?") → `Idle` +
///   `visible_idle` (positive evidence — retracts a stale hook Working);
/// - a transcript viewer / model picker overlay → `keep_previous`;
/// - nothing recognized → weak `Idle` at prompt; empty capture → `Unknown`.
fn claude_verdict(buffer: &str, title: Option<&str>) -> Verdict {
    if let Some(t) = title {
        if claude_title_working().is_match(t) {
            return Verdict::status(AgentStatus::Working);
        }
    }
    if buffer.trim().is_empty() {
        return Verdict::status(AgentStatus::Unknown);
    }
    let lines: Vec<&str> = buffer.lines().collect();
    let scanned = &lines[lines.len().saturating_sub(MAX_SCAN_LINES)..];
    let tail_lower = scanned.join("\n").to_ascii_lowercase();
    // Agent-owned overlays that show history, not live state: the transcript
    // viewer (ctrl+o) and the model picker. Nothing on these screens can say
    // whether the turn behind them runs, so the caller keeps what it had.
    if tail_lower.contains("showing detailed transcript")
        || (tail_lower.contains("select model") && tail_lower.contains("enter to set as default"))
    {
        return Verdict::keep_previous();
    }
    // Count non-blank lines from the bottom so the blank rows around the
    // input box don't shrink the live-tail window.
    let mut content_seen = 0usize;
    for line in scanned.iter().rev() {
        let lower = line.to_ascii_lowercase();
        // Strong, high-precision tells — matchable anywhere in the tail.
        if lower.contains(CLAUDE_INTERRUPT_HINT) || working_timer_tail().is_match(line) {
            return Verdict::status(AgentStatus::Working);
        }
        if CLAUDE_BLOCKER_CHROME.iter().any(|m| lower.contains(m)) {
            return Verdict::blocker();
        }
        if CLAUDE_WAITING_PROSE.iter().any(|m| lower.contains(m)) {
            return Verdict::status(AgentStatus::Waiting);
        }
        // A finished-turn summary ("✶ Cogitated for 8s") reuses a spinner
        // glyph, so rule it out before the bare-spinner tells below. Both it
        // and the interrupt line are positive idle evidence.
        if completed_line(&lower) || lower.contains(CLAUDE_INTERRUPTED) {
            return Verdict::idle_tell();
        }
        if !line.trim().is_empty() {
            content_seen += 1;
        }
        // Bare in-flight tells (no parenthetical), only within the bottom
        // LIVE_TAIL_LINES non-blank lines where a live spinner / tool line
        // sits.
        if content_seen <= LIVE_TAIL_LINES
            && (working_spinner_glyph().is_match(line)
                || working_spinner_tail().is_match(line)
                || working_tool_tail().is_match(line))
        {
            return Verdict::status(AgentStatus::Working);
        }
    }
    // No status line recognized → weak idle. The "✳ <summary>" title is NOT
    // an upgrade to positive idle: measured on 2.1.241 it shows mid-turn too
    // (tool phase and text streaming alike), so it can't distinguish the
    // streaming blind spot from a real prompt wait.
    Verdict::status(AgentStatus::Idle)
}

/// Codex's busy-spinner braille frames in the OSC pane title (herdr's char
/// set), surrounded by spaces or line edges so mid-word braille can't match.
fn codex_title_working() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| {
        regex::Regex::new(
            r"(?:^| )[\u{280B}\u{2819}\u{2839}\u{2838}\u{283C}\u{2834}\u{2826}\u{2827}\u{2807}\u{280F}](?: |$)",
        )
        .unwrap()
    })
}

/// Codex's live turn line: "• Working (13s • esc to interrupt) · …". Bullet-
/// led with a parenthetical holding the interrupt hint; the verb is left
/// free so a renamed spinner ("Thinking…") still matches. The line is
/// redrawn away the moment the turn ends, so it is a live tell.
fn codex_working_line() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| {
        regex::Regex::new(r"(?i)^\s*[\u{2022}\u{25e6}]\s+\w+ \([^)]*esc to interrupt").unwrap()
    })
}

/// Footers of Codex dialogs that block on the user — approval prompts,
/// question forms, and the hook trust gate ("… or esc to go back", which
/// herdr's manifest misses; deck has to see that one because deck's own
/// hook install is what causes it).
const CODEX_BLOCKER_FOOTERS: &[&str] = &[
    "press enter to confirm or esc to cancel",
    "press enter to confirm or esc to go back",
    "enter to submit answer",
    "enter to submit all",
    "allow command?",
];

/// Weaker per-line dialog tells (herdr's `weak_blocker`): cover an approval
/// dialog whose footer is truncated or phrased differently.
const CODEX_WEAK_BLOCKERS: &[&str] = &["[y/n]", "yes (y)", "do you want to", "would you like to"];

/// Classify Codex's pane. Same tiering as [`claude_verdict`]: title first
/// ("Action Required" → blocked; a braille spinner → working — both from
/// herdr's manifest), then full-screen modals, then a bottom-up scan of the
/// buffer tail, then the weak idle fallback.
fn codex_verdict(buffer: &str, title: Option<&str>) -> Verdict {
    if let Some(t) = title {
        let t_lower = t.to_ascii_lowercase();
        if t_lower.contains("action required") {
            return Verdict::blocker();
        }
        if codex_title_working().is_match(t) {
            return Verdict::status(AgentStatus::Working);
        }
    }
    if buffer.trim().is_empty() {
        return Verdict::status(AgentStatus::Unknown);
    }
    let lines: Vec<&str> = buffer.lines().collect();
    let scanned = &lines[lines.len().saturating_sub(MAX_SCAN_LINES)..];
    let tail_lower = scanned.join("\n").to_ascii_lowercase();
    // The transcript overlay (q to quit / scroll keys) shows history, not
    // live state.
    if tail_lower.contains("q to quit")
        && tail_lower.contains("to scroll")
        && tail_lower.contains("edit prev")
    {
        return Verdict::keep_previous();
    }
    // Full-screen modals, matched anywhere: the directory-trust question
    // (herdr's `trust_directory`) and the hook trust gate. Both paint over
    // the whole pane, so position carries no information.
    if tail_lower.contains("do you trust the contents of this directory?")
        || tail_lower.contains("trust all and continue")
        || tail_lower.contains("continue without trusting")
    {
        return Verdict::blocker();
    }
    let mut content_seen = 0usize;
    for line in scanned.iter().rev() {
        let lower = line.to_ascii_lowercase();
        if CODEX_BLOCKER_FOOTERS.iter().any(|m| lower.contains(m)) {
            return Verdict::blocker();
        }
        // The interrupt line persists in the transcript, but bottom-up
        // scanning means anything live below it (a new turn's Working line,
        // a fresh dialog) has already won.
        if lower
            .trim_start()
            .starts_with("\u{25a0} conversation interrupted")
        {
            return Verdict::idle_tell();
        }
        if CODEX_WEAK_BLOCKERS.iter().any(|m| lower.contains(m)) {
            return Verdict::status(AgentStatus::Waiting);
        }
        if !line.trim().is_empty() {
            content_seen += 1;
        }
        // The live turn line renders directly above the composer; gate it
        // like Claude's bare tells so stale-looking echoes far up can't sway
        // the verdict.
        if content_seen <= LIVE_TAIL_LINES && codex_working_line().is_match(line) {
            return Verdict::status(AgentStatus::Working);
        }
    }
    // Nothing recognized → weak idle. herdr upgrades this when the title is
    // non-empty and non-spinner (`osc_title_idle`), but through tmux a title
    // can be a stale shell-set one (a hostname) that Codex never touched, so
    // an unanchored title must not become positive-idle evidence here.
    Verdict::status(AgentStatus::Idle)
}

/// Classify a process by its `argv` string, returning the agent kind only
/// for an *interactive* invocation. Headless / non-interactive forms
/// return `None` (see `docs/agent-integration.md`).
fn classify(args: &str) -> Option<AgentKind> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    let arg0 = *tokens.first()?;
    let base = arg0.rsplit('/').next().unwrap_or(arg0);
    match base {
        "claude" => {
            let headless = args.contains("stream-json")
                || arg0.contains("/native-binary/claude")
                || tokens.iter().any(|t| *t == "-p" || *t == "--print");
            (!headless).then_some(AgentKind::Claude)
        }
        "codex" => {
            // Interactive by default (bare `codex`, `codex [PROMPT]`,
            // `resume`/`fork`). Only a known non-interactive subcommand (first
            // non-flag token) disqualifies it; defaulting to interactive avoids
            // misreading a prompt or flag value as a subcommand.
            let sub = tokens.iter().skip(1).find(|t| !t.starts_with('-')).copied();
            let non_interactive = matches!(
                sub,
                Some(
                    "exec"
                        | "e"
                        | "review"
                        | "login"
                        | "logout"
                        | "mcp"
                        | "mcp-server"
                        | "app-server"
                        | "remote-control"
                        | "app"
                        | "completion"
                        | "update"
                        | "doctor"
                        | "sandbox"
                        | "debug"
                        | "apply"
                        | "a"
                        | "plugin"
                        | "cloud"
                )
            );
            (!non_interactive).then_some(AgentKind::Codex)
        }
        _ => None,
    }
}

/// Locate the interactive agent (if any) in each pane.
///
/// `ps_output` is `ps -axo pid=,ppid=,args=` (local or via ssh). Per pane,
/// take the *shallowest* matching agent (breadth-first from the pane pid), so
/// sub-agent children don't double-count and a pane yields at most one agent.
/// Processes under no pane (e.g. IDE-extension headless instances) are never
/// reached, so they're excluded.
pub fn detect_agents(panes: &[PaneInfo], ps_output: &str) -> Vec<DetectedAgent> {
    let mut args_of: HashMap<u32, String> = HashMap::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in ps_output.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (
            it.next().and_then(|s| s.parse::<u32>().ok()),
            it.next().and_then(|s| s.parse::<u32>().ok()),
        ) else {
            continue;
        };
        // Whitespace-normalized args — fine, `classify` re-tokenizes.
        let args = it.collect::<Vec<_>>().join(" ");
        children.entry(ppid).or_default().push(pid);
        args_of.insert(pid, args);
    }

    let mut found = Vec::new();
    for p in panes {
        let mut queue: VecDeque<u32> = VecDeque::new();
        queue.push_back(p.pid);
        while let Some(pid) = queue.pop_front() {
            if let Some(kind) = args_of.get(&pid).and_then(|a| classify(a)) {
                found.push(DetectedAgent {
                    kind,
                    session: p.session.clone(),
                    window: p.window.clone(),
                    pane_id: p.pane_id.clone(),
                    // No buffer here; the gathering layer captures the pane
                    // and fills these in via `classify_verdict`/`merge_status`.
                    status: AgentStatus::Unknown,
                    keep_previous: false,
                });
                break; // one agent per pane
            }
            if let Some(kids) = children.get(&pid) {
                queue.extend(kids);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_claude_interactive_vs_headless() {
        // Interactive forms.
        assert_eq!(classify("claude"), Some(AgentKind::Claude));
        assert_eq!(
            classify("claude --dangerously-skip-permissions"),
            Some(AgentKind::Claude)
        );
        assert_eq!(
            classify("/Users/me/.local/bin/claude --resume abc"),
            Some(AgentKind::Claude)
        );
        // Headless forms are not counted.
        assert_eq!(classify("claude -p hello"), None);
        assert_eq!(classify("claude --print hi"), None);
        assert_eq!(
            classify("claude --output-format stream-json --verbose"),
            None
        );
        assert_eq!(
            classify("/Users/me/.cursor/extensions/x/resources/native-binary/claude --output-format stream-json"),
            None
        );
    }

    #[test]
    fn classify_codex_interactive_vs_subcommands() {
        // Interactive: bare, with a prompt, resume, fork — incl. the native
        // binary path the node wrapper spawns.
        assert_eq!(classify("codex"), Some(AgentKind::Codex));
        assert_eq!(classify("codex \"fix the bug\""), Some(AgentKind::Codex));
        assert_eq!(classify("codex resume"), Some(AgentKind::Codex));
        assert_eq!(classify("codex --model o3 fork"), Some(AgentKind::Codex));
        assert_eq!(
            classify(
                "/Users/me/.bun/.../@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/codex/codex"
            ),
            Some(AgentKind::Codex)
        );
        // Non-interactive subcommands are not counted.
        for sub in [
            "exec",
            "review",
            "mcp",
            "mcp-server",
            "app-server",
            "remote-control",
            "cloud",
            "login",
        ] {
            assert_eq!(
                classify(&format!("codex {sub} --flag")),
                None,
                "codex {sub}"
            );
        }
    }

    #[test]
    fn classify_ignores_non_agents() {
        assert_eq!(classify("-zsh"), None);
        assert_eq!(classify("/bin/zsh"), None);
        assert_eq!(classify("node /path/to/vite"), None);
        assert_eq!(classify("vim"), None);
        assert_eq!(classify(""), None);
    }

    fn pane(pid: u32, session: &str, window: &str) -> PaneInfo {
        PaneInfo {
            pid,
            session: session.to_string(),
            window: window.to_string(),
            pane_id: format!("%{pid}"),
            title: String::new(),
            window_activity: None,
            window_panes: None,
        }
    }

    #[test]
    fn detect_agents_one_per_pane_excludes_subagents_and_headless() {
        // pane 100: shell -> claude -> (sub-agent claude child, must NOT double-count)
        // pane 300: shell -> node wrapper -> native codex (matched at depth 2)
        // pane 500: shell -> vim (no agent)
        // pid 700: a headless claude NOT under any pane (ppid 1) -> excluded
        let ps = "\
100 1 -zsh
200 100 claude --dangerously-skip-permissions
250 200 claude --dangerously-skip-permissions
300 1 -zsh
400 300 node /Users/me/.bun/bin/codex
410 400 /Users/me/.bun/vendor/codex
500 1 -zsh
600 500 vim
700 1 /Users/me/.cursor/native-binary/claude --output-format stream-json";
        let panes = [
            pane(100, "deck", "main"),
            pane(300, "work", "agents"),
            pane(500, "work", "agents"),
        ];
        let agents = detect_agents(&panes, ps);
        assert_eq!(agents.len(), 2);
        // pane 100 -> claude, located at its session/window-name, with the
        // stable pane id carried for switching.
        assert_eq!(agents[0].kind, AgentKind::Claude);
        assert_eq!(agents[0].location(), "deck:main");
        assert_eq!(agents[0].pane_id, "%100");
        // pane 300 -> codex (matched two levels down the wrapper).
        assert_eq!(agents[1].kind, AgentKind::Codex);
        assert_eq!(agents[1].location(), "work:agents");
        assert_eq!(agents[1].pane_id, "%300");
        // pane 500 -> no agent (not in the list).
    }

    #[test]
    fn detect_agents_run_as_pane_root() {
        // Some setups exec the agent as the pane's command (pane_pid IS the
        // agent), with no intervening shell.
        let ps = "800 1 claude\n900 1 -zsh";
        let agents = detect_agents(&[pane(800, "s", "0"), pane(900, "s", "1")], ps);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].kind, AgentKind::Claude);
    }

    #[test]
    fn detect_agents_empty_inputs() {
        assert!(detect_agents(&[], "").is_empty());
        assert!(detect_agents(&[pane(1, "s", "0")], "").is_empty());
    }

    #[test]
    fn claude_classifier_reads_traffic_light_from_buffer() {
        // Working spinner: the "<verb>ing… (… esc to interrupt)" status line.
        let working = "✶ Cogitating… (12s · ↑ 3.2k tokens · esc to interrupt)";
        assert_eq!(
            classify_status(AgentKind::Claude, working),
            AgentStatus::Working
        );

        // Working spinner with a description between the verb and the tail — the
        // old "ing… (" marker missed this; the interrupt hint catches it.
        let described = "✱ Distilling findings into ai-patterns.md… (5m 21s · esc to interrupt)";
        assert_eq!(
            classify_status(AgentKind::Claude, described),
            AgentStatus::Working
        );

        // Working spinner whose interrupt hint is wrapped off — the live timer
        // tail "… (30s" still reads as working.
        let timer = "* Brewing… (30s · ↑ 1.2k tokens";
        assert_eq!(
            classify_status(AgentKind::Claude, timer),
            AgentStatus::Working
        );

        // Same, with ASCII "..." instead of the "…" glyph and the hint truncated.
        let ascii_dots = "* Distilling findings into ai-patterns.md... (5m 21s   17.6k toke";
        assert_eq!(
            classify_status(AgentKind::Claude, ascii_dots),
            AgentStatus::Working
        );

        // A bare thinking spinner (glyph + gerund + ellipsis, no parenthetical),
        // e.g. just after a turn starts before the timer renders.
        let spinner = "· Crunching…";
        assert_eq!(
            classify_status(AgentKind::Claude, spinner),
            AgentStatus::Working
        );

        // A spinner glyph leading a non-gerund, non-ellipsis status line — the
        // glyph alone signals an in-flight turn.
        let workflow = "✻ Waiting for 1 dynamic workflow to finish";
        assert_eq!(
            classify_status(AgentKind::Claude, workflow),
            AgentStatus::Working
        );

        // Realistic layout: the spinner sits a few lines above the input box, with
        // blank rows in between. Blank rows don't count toward the live-tail
        // window, so the glyph is still seen.
        let with_box = "✻ Waiting for 1 dynamic workflow to finish\n\n╭──────────────────────╮\n│ >                    │\n╰──────────────────────╯\n\n  ? for shortcuts";
        assert_eq!(
            classify_status(AgentKind::Claude, with_box),
            AgentStatus::Working
        );

        // A spinner glyph beyond the live-tail window (too far above the bottom)
        // is stale transcript, not the current state → idle.
        let stale_spinner = format!("✻ Cogitating\n{}\n│ >", "x\n".repeat(14));
        assert_eq!(
            classify_status(AgentKind::Claude, &stale_spinner),
            AgentStatus::Idle
        );

        // A bare tool line in flight (no parenthetical) on the bottom line.
        let tool = "some earlier output\nRunning command…";
        assert_eq!(
            classify_status(AgentKind::Claude, tool),
            AgentStatus::Working
        );

        // A "… (ctrl+o to expand)" tool-result tail is NOT a live timer → not
        // working on its own (no interrupt hint, paren isn't a duration).
        let collapsed =
            "⏺ Read(config.yaml)\n  ⎿ Read 1 file… (ctrl+o to expand)\n│ > │\n? for shortcuts";
        assert_eq!(
            classify_status(AgentKind::Claude, collapsed),
            AgentStatus::Idle
        );

        // Idle at the prompt, nothing pending.
        let idle = "╭───────────╮\n│ > │\n╰───────────╯\n? for shortcuts";
        assert_eq!(classify_status(AgentKind::Claude, idle), AgentStatus::Idle);

        // A permission dialog → waiting on the user.
        let prompt = "Do you want to proceed?\n❯ 1. Yes\n  2. No";
        assert_eq!(
            classify_status(AgentKind::Claude, prompt),
            AgentStatus::Waiting
        );

        // A finished turn ("…ed for <n>") below a stale spinner reads as idle —
        // bottom-up wins.
        let done = "✶ Cogitating… (3s · esc to interrupt)\n● Done\n✶ Cogitated for 8s";
        assert_eq!(classify_status(AgentKind::Claude, done), AgentStatus::Idle);

        // Empty capture → unknown.
        assert_eq!(
            classify_status(AgentKind::Claude, "   "),
            AgentStatus::Unknown
        );
    }

    /// Mid-turn plain-text streaming: no spinner, no interrupt hint — just
    /// content and the composer (real capture, 2026-08-24). The buffer alone
    /// is blind here; the spinning pane title is what carries Working.
    const CLAUDE_STREAMING: &str = "  249\n  250\n  251\n\
────────────────────────────────\n\
❯ \n\
────────────────────────────────\n\
  ⏸ manual mode on · ? for shortcuts · ← for agents";

    #[test]
    fn claude_title_covers_the_streaming_blind_spot() {
        // Buffer only → weak idle (the documented blind spot): no positive
        // idle evidence, so a hook/activity source may still say Working.
        let blind = classify_verdict(AgentKind::Claude, CLAUDE_STREAMING, None);
        assert_eq!(blind.status, AgentStatus::Idle);
        assert!(!blind.visible_idle && !blind.visible_blocker && !blind.keep_previous);

        // A braille or half-circle spinner title outranks the blind buffer.
        for title in ["⠧ fixing the flaky test", "◐ fixing the flaky test"] {
            assert_eq!(
                classify_verdict(AgentKind::Claude, CLAUDE_STREAMING, Some(title)).status,
                AgentStatus::Working,
                "title {title:?}"
            );
        }

        // The "✳ <summary>" title upgrades nothing: measured on 2.1.241 it
        // shows during a running turn too, so it is not idle evidence.
        let rest = classify_verdict(AgentKind::Claude, CLAUDE_STREAMING, Some("✳ deck"));
        assert_eq!(rest.status, AgentStatus::Idle);
        assert!(!rest.visible_idle);

        // A stale shell-set title (hostname) upgrades nothing either.
        let stale = classify_verdict(AgentKind::Claude, CLAUDE_STREAMING, Some("mybox.local"));
        assert_eq!(stale.status, AgentStatus::Idle);
        assert!(!stale.visible_idle);
    }

    #[test]
    fn claude_interrupt_and_completion_are_positive_idle() {
        // Esc interrupt fires no lifecycle hook at all, so this transcript
        // line is what retracts a stale hook-reported Working.
        let interrupted = "  252\n  ⎿  Interrupted · What should Claude do instead?\n\n❯ \n\
  ⏸ manual mode on · ? for shortcuts";
        let v = classify_verdict(AgentKind::Claude, interrupted, None);
        assert_eq!(v.status, AgentStatus::Idle);
        assert!(v.visible_idle);

        // Same for the finished-turn summary.
        let done = classify_verdict(AgentKind::Claude, "⏺ pong\n\n✻ Churned for 6s\n\n❯ ", None);
        assert_eq!(done.status, AgentStatus::Idle);
        assert!(done.visible_idle);
    }

    #[test]
    fn claude_dialogs_are_visible_blockers() {
        // Workspace-trust dialog (real capture).
        let trust = " Accessing workspace:\n\n /private/tmp/dh/work\n\n\
 Claude Code'll be able to read, edit, and execute files here.\n\n\
 ❯ 1. Yes, I trust this folder\n   2. No, exit\n\n Enter to confirm · Esc to cancel";
        let v = classify_verdict(AgentKind::Claude, trust, None);
        assert_eq!(v.status, AgentStatus::Waiting);
        assert!(v.visible_blocker);

        // Bash-permission footer alone (question scrolled off).
        let footer = "   mkdir -p /tmp/probe\n\n Esc to cancel · Tab to amend · ctrl+e to explain";
        assert!(classify_verdict(AgentKind::Claude, footer, None).visible_blocker);
    }

    #[test]
    fn claude_question_prose_is_weak_waiting_not_a_blocker() {
        // A finished turn that ends by asking the user something: Waiting at
        // rest, but only weakly. The line lingers in the transcript, so it
        // must not claim the hard `visible_blocker` that real dialog chrome
        // gets -- a live source has to be able to outrank it while the next
        // turn streams past it. What outranks it arrives with the evidence
        // layers above this one; here the verdict just has to leave room.
        let prose = "⏺ Done with the refactor. Do you want me to run the tests?\n\n❯ \n\
  ⏸ manual mode on · ? for shortcuts";
        let v = classify_verdict(AgentKind::Claude, prose, None);
        assert_eq!(v.status, AgentStatus::Waiting);
        assert!(!v.visible_blocker);

        // Real dialog chrome stays a hard blocker.
        let dialog = " Do you want to proceed?\n ❯ 1. Yes\n   2. No";
        assert!(classify_verdict(AgentKind::Claude, dialog, None).visible_blocker);
    }

    #[test]
    fn claude_viewer_overlays_keep_previous_status() {
        let transcript = "  ⎿  Ran 1 shell command\n\n\
  showing detailed transcript · ctrl+o to toggle";
        let v = classify_verdict(AgentKind::Claude, transcript, None);
        assert!(v.keep_previous);
        assert_eq!(v.status, AgentStatus::Unknown);

        let picker = " Select model\n ❯ 1. Default\n   2. Opus\n\n\
 enter to set as default · esc to cancel";
        assert!(classify_verdict(AgentKind::Claude, picker, None).keep_previous);
    }

    /// Codex approval dialog (real capture, 2026-08-24).
    const CODEX_APPROVAL: &str = "  Would you like to run the following command?\n\n\
  Environment: local\n\n  $ curl -sI https://example.com | head -1\n\n\
› 1. Yes, proceed (y)\n  2. Yes, and don't ask again for commands like this (p)\n\
  3. No, and tell Codex what to do differently (esc)\n\n\
  Press enter to confirm or esc to cancel";

    #[test]
    fn codex_classifier_reads_traffic_light_from_buffer() {
        // Live turn line, with and without the background-terminal suffix.
        let working = "• Running it now; it should finish in about 40 seconds.\n\n\
• Working (13s • esc to interrupt) · 1 background terminal running · /ps to view · /stop to close\n\n\
› Ask Codex to do anything\n\n  gpt-5.6-sol low · /private/tmp/dc/work";
        assert_eq!(
            classify_status(AgentKind::Codex, working),
            AgentStatus::Working
        );
        assert_eq!(
            classify_status(AgentKind::Codex, "• Working (3s • esc to interrupt)\n› "),
            AgentStatus::Working
        );

        // Approval dialog → waiting, visibly.
        let v = classify_verdict(AgentKind::Codex, CODEX_APPROVAL, None);
        assert_eq!(v.status, AgentStatus::Waiting);
        assert!(v.visible_blocker);

        // Esc interrupt → positive idle (no hook event fires for it).
        let interrupted = "✗ You canceled the request to run curl -sI https://example.com | head -1\n\n\
■ Conversation interrupted - tell the model what to do differently. Something went wrong? Hit `/feedback` to report the issue.\n\n\
› Ask Codex to do anything\n\n  gpt-5.6-sol low · /private/tmp/dc/work";
        let v = classify_verdict(AgentKind::Codex, interrupted, None);
        assert_eq!(v.status, AgentStatus::Idle);
        assert!(v.visible_idle);

        // A stale interrupt line above a live turn: bottom-up, the lower
        // Working line wins (real layout from the experiment).
        let resumed = "■ Conversation interrupted - tell the model what to do differently.\n\n\
› run this shell command: touch /tmp/probe\n\n\
• Working (0s • esc to interrupt) · 1 background terminal running\n\n\
› Ask Codex to do anything";
        assert_eq!(
            classify_status(AgentKind::Codex, resumed),
            AgentStatus::Working
        );

        // Idle at the composer; empty capture → unknown.
        assert_eq!(
            classify_status(
                AgentKind::Codex,
                "› Ask Codex to do anything\n\n  gpt-5.6-sol low"
            ),
            AgentStatus::Idle
        );
        assert_eq!(classify_status(AgentKind::Codex, " "), AgentStatus::Unknown);
    }

    #[test]
    fn codex_trust_screens_are_visible_blockers() {
        // Directory-trust question (herdr's `trust_directory` shape).
        let dir = "> You are in /tmp/dc/work\n\n\
  Do you trust the contents of this directory?\n\n\
› 1. Yes, continue\n  2. No, quit";
        assert!(classify_verdict(AgentKind::Codex, dir, None).visible_blocker);

        // The hook trust gate — deck's own install causes this screen, so
        // deck must be able to see it (herdr's manifest has no rule for it).
        let gate = "  Hooks need review\n  11 hooks are new or changed.\n\
  Hooks can run outside the sandbox after you trust them.\n\n\
› 1. Review hooks\n  2. Trust all and continue\n  3. Continue without trusting (hooks won't run)\n\n\
  Press enter to confirm or esc to go back";
        assert!(classify_verdict(AgentKind::Codex, gate, None).visible_blocker);
    }

    #[test]
    fn codex_title_tiers() {
        let idle_buffer = "› Ask Codex to do anything\n\n  gpt-5.6-sol low";
        // "Action Required" title → blocked, ahead of everything.
        let v = classify_verdict(
            AgentKind::Codex,
            idle_buffer,
            Some("Action Required · codex"),
        );
        assert_eq!(v.status, AgentStatus::Waiting);
        assert!(v.visible_blocker);
        // Braille spinner title → working, even over a blind buffer.
        assert_eq!(
            classify_verdict(AgentKind::Codex, idle_buffer, Some("⠼ build the thing")).status,
            AgentStatus::Working
        );
        // An unanchored title (stale shell title) must NOT become positive
        // idle: through tmux we can't know Codex ever set it.
        let stale = classify_verdict(AgentKind::Codex, idle_buffer, Some("mybox.local"));
        assert_eq!(stale.status, AgentStatus::Idle);
        assert!(!stale.visible_idle);
    }

    #[test]
    fn activity_fresh_needs_a_sole_pane_and_both_clocks() {
        // The window must be the agent's alone.
        assert_eq!(activity_fresh(Some(100), Some(2), Some(100)), None);
        assert_eq!(activity_fresh(Some(100), None, Some(100)), None);
        // Both clock readings must exist.
        assert_eq!(activity_fresh(None, Some(1), Some(100)), None);
        assert_eq!(activity_fresh(Some(100), Some(1), None), None);
        // Fresh within the window (incl. a slightly-ahead stamp), stale past it.
        assert_eq!(activity_fresh(Some(100), Some(1), Some(101)), Some(true));
        assert_eq!(activity_fresh(Some(101), Some(1), Some(100)), Some(true));
        assert_eq!(
            activity_fresh(
                Some(100),
                Some(1),
                Some(100 + ACTIVITY_FRESH_WINDOW_SECS + 1)
            ),
            Some(false)
        );
    }

    #[test]
    fn merge_status_orders_the_evidence() {
        let weak = Verdict::status(AgentStatus::Idle);
        // Streaming blind spot: weak idle + fresh output → Working.
        assert_eq!(merge_status(weak, Some(true)), AgentStatus::Working);
        // No activity signal → the weak reading passes through.
        assert_eq!(merge_status(weak, None), AgentStatus::Idle);
        assert_eq!(merge_status(weak, Some(false)), AgentStatus::Idle);

        // A visible dialog outranks fresh output (the dialog paint IS the
        // fresh output).
        assert_eq!(
            merge_status(Verdict::blocker(), Some(true)),
            AgentStatus::Waiting
        );

        // A previous turn's completed line lingers on screen while the next
        // turn streams: fresh output disproves the idle remnant.
        assert_eq!(
            merge_status(Verdict::idle_tell(), Some(true)),
            AgentStatus::Working
        );
        // …but once output stops, the positive idle stands.
        assert_eq!(
            merge_status(Verdict::idle_tell(), Some(false)),
            AgentStatus::Idle
        );
        assert_eq!(merge_status(Verdict::idle_tell(), None), AgentStatus::Idle);

        // Viewer overlays resolve at the stateful layer (keep_previous flag
        // on the DetectedAgent); here they are Unknown.
        assert_eq!(
            merge_status(Verdict::keep_previous(), Some(true)),
            AgentStatus::Unknown
        );

        // Weak waiting (dialog-ish text, no live footer) survives unless
        // output is actually flowing.
        let weak_wait = Verdict::status(AgentStatus::Waiting);
        assert_eq!(merge_status(weak_wait, None), AgentStatus::Waiting);
        assert_eq!(merge_status(weak_wait, Some(true)), AgentStatus::Working);
    }

    #[test]
    fn codex_transcript_overlay_keeps_previous() {
        let overlay = "  … transcript content …\n\n\
  ↑/↓ to scroll · pgup/pgdn to page · q to quit · esc to edit prev";
        let v = classify_verdict(AgentKind::Codex, overlay, None);
        assert!(v.keep_previous);
        assert_eq!(v.status, AgentStatus::Unknown);
    }
}
