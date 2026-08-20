//! Agents-tab "Summary" generation: capture each detected agent's pane
//! buffer, stitch them into one XML-flavored prompt, then run the configured
//! Claude Code or Codex CLI headlessly for a plain-text summary.
//!
//! The prompt is a user-editable template (persisted via `crate::config`)
//! with `{{SESSIONS}}` where the per-pane `<session>` blocks splice in.
//! Everything runs off the UI thread, on the worker
//! `App::start_summary_generation` spawns.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::summary_card::SummaryAgent;
use crate::worker::Cancel;

/// Hard ceiling on a single summary run, so a hung agent CLI can't
/// pin `SummaryState::Generating` forever: past this the child is killed and
/// an error surfaced (card shows failure, Generate re-enables). Generous
/// because a multi-pane summary on a slow model legitimately takes a while.
pub const SUMMARY_TIMEOUT: Duration = Duration::from_secs(90);

/// How often the child wait loop wakes to check the deadline and the
/// cancel flag. Small enough that an Esc/cancel kills the child promptly.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Sentinel error returned when a run is cancelled (vs. genuinely failed).
/// The run loop recognizes it and leaves the (already-restored) card state
/// alone rather than overwriting it with an `Error` card.
pub const CANCELLED_MSG: &str = "summary cancelled";

/// Bumped whenever `DEFAULT_SUMMARY_PROMPT` changes. On startup a config with
/// an older version is refreshed to the new default (see `Config::load`), so
/// improvements reach existing users — at the cost of resetting a hand-edited
/// prompt when the default moves.
pub const DEFAULT_SUMMARY_PROMPT_VERSION: u32 = 3;

/// Marker the template replaces with the rendered `<session>` blocks. A
/// template missing it still works — the blocks are appended instead.
pub const PROMPT_PLACEHOLDER: &str = "{{SESSIONS}}";

/// Markers that a captured pane is showing deck's own UI (nested deck), not
/// agent work: any supported Sessions header or the legacy footer version
/// label. Keep all icon styles here because this module intentionally does not
/// depend on the UI layer. Such a pane is marked nested and omitted.
const DECK_UI_MARKERS: &[&str] = &[
    "▤ Sessions",        // default Unicode icon style
    "# Sessions",        // strict ASCII fallback
    "\u{e795} Sessions", // opt-in Nerd Font style
    "[$ deck v",         // legacy footer marker
];

fn is_nested_deck(buffer: &str) -> bool {
    DECK_UI_MARKERS.iter().any(|m| buffer.contains(m))
}

/// The default, user-overridable summary prompt. `{{SESSIONS}}` is filled
/// with one `<session id="…">…</session>` block per agent pane.
pub const DEFAULT_SUMMARY_PROMPT: &str = "\
You are deck, a tmux session manager. Several coding agents (Claude Code or \
Codex) are running in tmux panes. Below is the recent terminal buffer of each \
agent's pane, one <session> block per pane, where the id is its tmux \
session:window.pane location (and host=\"…\" marks a remote host).

{{SESSIONS}}

Summarize the current state across all sessions: what each agent is working \
on, which are actively running versus idle or waiting for input, and any \
errors or blockers. Then suggest the most useful next action. Refer to \
sessions by their id. Be concise: if one sentence says it clearly, use one \
sentence.

Formatting: plain prose with short paragraphs. You may use `## headings`, \
**bold** for emphasis, and `inline code` for commands, paths, and ids. Do NOT \
use tables, code fences (```), bullet lists, blockquotes, or links; only \
those three inline markers are rendered. Do NOT use dash punctuation: no em \
dashes, en dashes, or hyphens to join or separate clauses; use a comma or a \
separate sentence instead.";

/// One agent's pane to capture for a summary (Agents tab only).
#[derive(Debug, Clone)]
pub struct SummaryPane {
    /// `None` = local, `Some(host)` = a remote ssh host.
    pub host: Option<String>,
    /// Display id for the `<session>` block — the agent's `session:window`
    /// location.
    pub id: String,
    /// The tmux `-t` target used to capture the buffer: the agent's stable
    /// `%N` pane handle.
    pub target: String,
}

/// A captured pane's buffer plus the identity for its `<session>` block.
#[derive(Debug, Clone)]
pub struct PaneCapture {
    pub host: Option<String>,
    pub id: String,
    pub content: String,
}

/// The default model for summaries — fast and cheap, which suits
/// condensing terminal buffers. Overridable via `config.summary_model`.
pub const DEFAULT_SUMMARY_MODEL: &str = "haiku";

/// Display label for the configured summary language (empty → "Default").
/// The user types the language freely in the settings editor.
pub fn language_label(lang: &str) -> &str {
    if lang.trim().is_empty() {
        "Default"
    } else {
        lang
    }
}

/// Capture every pane, build the prompt from `template`, and run the selected
/// agent CLI. `model` applies to Claude; Codex follows its configured default.
/// `Err` carries a short, user-facing reason for the card.
pub fn generate(
    panes: &[SummaryPane],
    template: &str,
    agent: SummaryAgent,
    model: &str,
    language: &str,
    cancel: &Cancel,
) -> Result<String, String> {
    if panes.is_empty() {
        return Err("Nothing to summarize.".to_string());
    }
    // Remote panes are captured one batched ssh hop per HOST (not one hop
    // per pane — N sequential 5s-budget roundtrips added up fast); local
    // captures stay per-pane, they're cheap tmux IPC.
    let mut by_host: std::collections::HashMap<&str, Vec<String>> =
        std::collections::HashMap::new();
    for a in panes {
        if let Some(host) = &a.host {
            by_host.entry(host).or_default().push(a.target.clone());
        }
    }
    let remote_buffers: std::collections::HashMap<&str, _> = by_host
        .into_iter()
        .map(|(host, ids)| (host, crate::remote_tmux::capture_panes(host, &ids)))
        .collect();
    let captures: Vec<PaneCapture> = panes
        .iter()
        .map(|a| {
            let raw = match &a.host {
                None => crate::tmux::capture_pane(&a.target).unwrap_or_default(),
                Some(host) => remote_buffers
                    .get(host.as_str())
                    .and_then(|m| m.get(&a.target))
                    .cloned()
                    .unwrap_or_default(),
            };
            // A pane showing deck's own UI is a nested deck, not an agent's
            // work — don't feed deck's chrome back into the summary; mark it
            // nested and omit the buffer.
            let content = if is_nested_deck(&raw) {
                "[content omitted — nested deck UI detected]".to_string()
            } else {
                raw
            };
            PaneCapture {
                host: a.host.clone(),
                id: a.id.clone(),
                content,
            }
        })
        .collect();
    let mut prompt = build_prompt(template, &captures);
    // A non-default language just appends a closing instruction, per the
    // user's request — the template itself stays language-agnostic.
    if !language.trim().is_empty() {
        prompt.push_str(&format!("\n\nGive me the response in {}.", language.trim()));
    }
    let started = SystemTime::now();
    let result = run_agent(agent, &prompt, model, cancel);
    // Best-effort: a logging failure must not fail the generation.
    write_log(panes, agent, model, &prompt, &result, started);
    result
}

/// Directory for per-generation debug logs, when logging is enabled (see
/// `write_log`). Under `~/.cache/deck/`, not `/tmp`, because these files embed
/// captured pane buffers (possibly tokens or on-screen secrets) and must not
/// sit world-readable in a shared temp dir.
pub fn log_dir() -> PathBuf {
    crate::config::home_dir()
        .join(".cache")
        .join("deck")
        .join("summary")
}

/// Summary logging is opt-in via the `DECK_SUMMARY_LOG` env var, because the
/// record embeds captured pane buffers. Any value (even empty) enables it.
fn summary_logging_enabled() -> bool {
    std::env::var_os("DECK_SUMMARY_LOG").is_some()
}

/// Max debug logs to retain; older entries are pruned on each write.
const MAX_SUMMARY_LOGS: usize = 20;

/// Append a record of one generation: timestamp, deck version, model,
/// sessions captured, full input prompt, and response (or error).
///
/// Opt-in (`DECK_SUMMARY_LOG`) because the record embeds captured pane
/// buffers; when enabled, entries go under `~/.cache/deck/` owner-only and are
/// capped. Best-effort — all IO failures are swallowed.
fn write_log(
    panes: &[SummaryPane],
    agent: SummaryAgent,
    model: &str,
    prompt: &str,
    result: &Result<String, String>,
    started: SystemTime,
) {
    let now = SystemTime::now();
    let millis = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis();
    let secs = millis / 1000;
    let elapsed_ms = now
        .duration_since(started)
        .unwrap_or(Duration::ZERO)
        .as_millis();

    let sessions = panes
        .iter()
        .map(|a| match &a.host {
            Some(h) => format!("{h}:{}", a.id),
            None => a.id.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let (status, response) = match result {
        Ok(text) => ("ok", text.as_str()),
        Err(e) => ("error", e.as_str()),
    };
    let model = match agent {
        SummaryAgent::Claude if !model.trim().is_empty() => model.trim(),
        SummaryAgent::Claude => "(claude default)",
        SummaryAgent::Codex => "(codex default)",
    };

    let body = format!(
        "# deck summary log\n\n\
         - time: {secs} (unix epoch seconds)\n\
         - deck version: {version}\n\
         - agent: {agent}\n\
         - model: {model}\n\
         - status: {status}\n\
         - duration_ms: {elapsed_ms}\n\
         - sessions ({count}): {sessions}\n\n\
         ## input prompt\n\n{prompt}\n\n\
         ## response\n\n{response}\n",
        version = env!("CARGO_PKG_VERSION"),
        agent = agent.label(),
        count = panes.len(),
    );

    write_log_entry(&log_dir(), summary_logging_enabled(), millis, &body);
}

/// Write one log entry under `dir` (owner-only) and prune old entries — but
/// only when `enabled`. Split out so the gating, perms, and pruning are
/// testable without touching env vars or `$HOME`.
fn write_log_entry(dir: &std::path::Path, enabled: bool, millis: u128, body: &str) {
    if !enabled {
        return;
    }
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Restrict the directory to the owner (0700). Best-effort.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = dir.join(format!("summary-{millis}.md"));
    if write_owner_only(&path, body).is_err() {
        return;
    }
    prune_logs(dir, MAX_SUMMARY_LOGS);
}

/// Create `path` mode 0600 and write `body`, so the captured buffers are
/// owner-readable only (closes the world-readable `/tmp` hole).
#[cfg(unix)]
fn write_owner_only(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(body.as_bytes())
}

#[cfg(not(unix))]
fn write_owner_only(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    std::fs::write(path, body)
}

/// Keep only the newest `keep` `summary-*.md` entries in `dir`, deleting the
/// rest. Filenames embed a monotonic, constant-width millis counter, so
/// lexicographic order is chronological — no `metadata()` calls needed.
fn prune_logs(dir: &std::path::Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("summary-") && n.ends_with(".md"))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort();
    let remove = files.len() - keep;
    for p in files.into_iter().take(remove) {
        let _ = std::fs::remove_file(p);
    }
}

/// Splice the rendered `<session>` blocks into `template` at
/// `{{SESSIONS}}`; if the marker is absent, append them.
pub fn build_prompt(template: &str, captures: &[PaneCapture]) -> String {
    let mut blocks = String::new();
    for c in captures {
        let host_attr = match &c.host {
            Some(h) => format!(" host=\"{}\"", attr_escape(h)),
            None => String::new(),
        };
        blocks.push_str(&format!(
            "<session id=\"{}\"{}>\n{}\n</session>\n\n",
            attr_escape(&c.id),
            host_attr,
            c.content.trim_end_matches('\n'),
        ));
    }
    let blocks = blocks.trim_end().to_string();
    if template.contains(PROMPT_PLACEHOLDER) {
        template.replace(PROMPT_PLACEHOLDER, &blocks)
    } else {
        format!("{}\n\n{}", template.trim_end(), blocks)
    }
}

/// Minimal escaping for an XML attribute value (`&`, `<`, `"`). Pane
/// *content* is left raw — it's prose for an LLM, not strict XML.
fn attr_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

/// Kill `child` and reap it so it can't linger as a zombie. Best-effort:
/// the process may already have exited, in which case `kill` is a no-op
/// error we ignore; `wait` then reaps whatever's left.
fn kill_and_reap(child: &mut Child) {
    // Child leads its own process group; signal the whole group to also kill
    // its subprocesses (MCP servers, tools) rather than orphaning them.
    #[cfg(unix)]
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Run the selected CLI with `prompt` on stdin and return trimmed stdout.
/// Stdin avoids the `ARG_MAX` limit a large multi-pane prompt could hit.
///
/// Bounded and cancellable: the child is polled against [`SUMMARY_TIMEOUT`]
/// and `cancel`, and killed + reaped on either. Every error path (stdin
/// write/EPIPE, spawn, timeout, cancel) also kills + reaps, so no zombie leaks.
pub fn run_agent(
    agent: SummaryAgent,
    prompt: &str,
    model: &str,
    cancel: &Cancel,
) -> Result<String, String> {
    let (cmd, program) = summary_command(agent, model);
    run_command(cmd, program, prompt, cancel)
}

fn summary_command(agent: SummaryAgent, model: &str) -> (Command, &'static str) {
    match agent {
        SummaryAgent::Claude => {
            let mut cmd = Command::new("claude");
            cmd.arg("-p");
            if !model.trim().is_empty() {
                cmd.arg("--model").arg(model.trim());
            }
            (cmd, "claude")
        }
        SummaryAgent::Codex => {
            let mut cmd = Command::new("codex");
            // Ephemeral avoids adding a summary-only run to session history;
            // read-only prevents a summarizer from mutating the workspace.
            // `-` makes stdin the prompt, matching Claude's ARG_MAX-safe path.
            cmd.args([
                "exec",
                "--ephemeral",
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
                "--color",
                "never",
                "-",
            ]);
            (cmd, "codex")
        }
    }
}

/// Drive an already-configured command to completion with `prompt` on stdin,
/// bounded by [`SUMMARY_TIMEOUT`] and `cancel`. Split out of [`run_agent`] so
/// the kill/cancel/timeout paths can be tested against a stub binary.
fn run_command(
    mut cmd: Command,
    program: &str,
    prompt: &str,
    cancel: &Cancel,
) -> Result<String, String> {
    // Own process group so a timeout/cancel kill reaches the subprocesses it
    // spawns (MCP servers, tools); `kill_and_reap` signals the whole group.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not launch {program}: {e}"))?;

    // Write the prompt, then drop stdin so the CLI sees EOF and starts.
    // A write failure (e.g. the CLI exited early → EPIPE) must still
    // kill + reap the child, or it leaks as a zombie.
    {
        let mut stdin = match child.stdin.take() {
            Some(s) => s,
            None => {
                kill_and_reap(&mut child);
                return Err(format!("{program} stdin unavailable"));
            }
        };
        if let Err(e) = stdin.write_all(prompt.as_bytes()) {
            drop(stdin);
            kill_and_reap(&mut child);
            return Err(format!("writing prompt to {program}: {e}"));
        }
    }

    // Poll against the deadline and cancel flag rather than blocking forever
    // in `wait_with_output`.
    let deadline = Instant::now() + SUMMARY_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if cancel.is_cancelled() {
                    kill_and_reap(&mut child);
                    return Err(CANCELLED_MSG.to_string());
                }
                if Instant::now() >= deadline {
                    kill_and_reap(&mut child);
                    return Err(format!(
                        "{program} timed out after {}s",
                        SUMMARY_TIMEOUT.as_secs()
                    ));
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => {
                kill_and_reap(&mut child);
                return Err(format!("waiting on {program}: {e}"));
            }
        }
    }

    let out = child
        .wait_with_output()
        .map_err(|e| format!("waiting on {program}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        // Agent CLIs report failures (auth, invalid model, runtime
        // errors) on stdout as often as stderr, so surface whichever
        // carries text — otherwise the card would show a bare exit code.
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = [stderr.trim(), stdout.trim()]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or("");
        let code = out
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        return Err(if detail.is_empty() {
            format!("{program} exited with code {code} (no output)")
        } else {
            format!("{program} failed (exit {code}): {detail}")
        });
    }
    Ok(stdout.trim().to_string())
}

#[cfg(test)]
#[path = "../../tests/unit/infra/summary.rs"]
mod tests;
