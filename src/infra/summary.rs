//! Agents-tab "Summary" generation: capture each detected agent's pane
//! buffer, stitch them into one XML-flavored prompt, and run the `claude`
//! CLI headlessly (`claude -p`, prompt on stdin) to produce a plain-text
//! summary.
//!
//! The prompt is a user-editable template (persisted to the config file,
//! see `crate::config`); `{{SESSIONS}}` is where the per-pane `<session>`
//! blocks are spliced in. Everything here runs off the UI thread, on the
//! worker `App::start_summary_generation` spawns.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Bumped whenever `DEFAULT_SUMMARY_PROMPT` changes. On startup a config
/// carrying an older version is refreshed to the new default (see
/// `Config::load`), so template improvements ship to existing users — at
/// the cost of resetting a hand-edited prompt when the default moves.
pub const DEFAULT_SUMMARY_PROMPT_VERSION: u32 = 3;

/// Marker the template replaces with the rendered `<session>` blocks. A
/// template missing it still works — the blocks are appended instead.
pub const PROMPT_PLACEHOLDER: &str = "{{SESSIONS}}";

/// Markers that mean a captured pane is showing deck's own UI (a nested
/// deck) rather than an agent's work: the sidebar's Projects glyph and the
/// footer's "[$ deck v…]" version label. The footer label is plain ASCII,
/// so it survives `capture-pane` reliably even where the glyph doesn't.
/// Such a pane is marked nested and omitted from the summary.
const DECK_UI_MARKERS: &[&str] = &["\u{e795}", "[$ deck v"];

/// Whether a captured buffer is a nested deck UI (see `DECK_UI_MARKERS`).
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

/// One agent pane to capture, as snapshotted from the Agents tab.
#[derive(Debug, Clone)]
pub struct AgentPane {
    /// `None` = local, `Some(host)` = a remote ssh host.
    pub host: Option<String>,
    /// Display id for the `<session>` block — the `session:window.pane`
    /// location (`DetectedAgent::location`).
    pub id: String,
    /// Stable `%N` pane handle used to capture the buffer.
    pub pane_id: String,
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

/// Capture every pane, build the prompt from `template`, and run `claude`
/// with `model` (empty = the user's Claude Code default). `Err` carries a
/// short, user-facing reason (no agents, claude missing, non-zero exit)
/// for the card to display.
pub fn generate(
    agents: &[AgentPane],
    template: &str,
    model: &str,
    language: &str,
) -> Result<String, String> {
    if agents.is_empty() {
        return Err("No agents detected to summarize.".to_string());
    }
    let captures: Vec<PaneCapture> = agents
        .iter()
        .map(|a| {
            let raw = match &a.host {
                None => crate::tmux::capture_pane(&a.pane_id),
                Some(host) => crate::remote_tmux::capture_pane(host, &a.pane_id),
            }
            .unwrap_or_default();
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
        prompt.push_str(&format!(
            "\n\nGive me the response in {}.",
            language.trim()
        ));
    }
    let started = SystemTime::now();
    let result = run_claude(&prompt, model);
    // Best-effort: a logging failure must not fail the generation.
    write_log(agents, model, &prompt, &result, started);
    result
}

/// Directory holding one debug log per summary generation, when logging is
/// enabled (see `write_log`). Under `~/.cache/deck/` — not `/tmp` — because
/// these files embed each captured pane buffer (which can hold tokens or
/// other on-screen secrets) and must not sit world-readable in a shared
/// temp dir.
pub fn log_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
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

/// Append a record of one generation: timestamp, deck version, model, the
/// sessions captured, the full input prompt, and the response (or error).
///
/// Logging is **opt-in** (`DECK_SUMMARY_LOG`) because the record embeds each
/// captured pane buffer; when enabled, entries go under `~/.cache/deck/`
/// owner-only and are capped. Best-effort — all IO failures are swallowed.
fn write_log(
    agents: &[AgentPane],
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

    let sessions = agents
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
    let model = if model.trim().is_empty() {
        "(claude default)"
    } else {
        model.trim()
    };

    let body = format!(
        "# deck summary log\n\n\
         - time: {secs} (unix epoch seconds)\n\
         - deck version: {version}\n\
         - model: {model}\n\
         - status: {status}\n\
         - duration_ms: {elapsed_ms}\n\
         - sessions ({count}): {sessions}\n\n\
         ## input prompt\n\n{prompt}\n\n\
         ## response\n\n{response}\n",
        version = env!("CARGO_PKG_VERSION"),
        count = agents.len(),
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

/// Run `claude -p` with `prompt` on stdin and return its trimmed stdout.
/// Headless print mode reads the prompt from stdin (it's "useful for
/// pipes"); stdin avoids `ARG_MAX` limits a large multi-pane prompt could
/// hit as an argv. `model` (e.g. "haiku") is passed via `--model`; empty
/// falls back to the user's Claude Code default.
pub fn run_claude(prompt: &str, model: &str) -> Result<String, String> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p");
    if !model.trim().is_empty() {
        cmd.arg("--model").arg(model.trim());
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not launch claude: {e}"))?;

    // Write the prompt, then drop stdin so claude sees EOF and starts.
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "claude stdin unavailable".to_string())?;
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| format!("writing prompt to claude: {e}"))?;
    }

    let out = child
        .wait_with_output()
        .map_err(|e| format!("waiting on claude: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        // `claude -p` reports failures (auth, invalid model, runtime
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
            format!("claude exited with code {code} (no output)")
        } else {
            format!("claude failed (exit {code}): {detail}")
        });
    }
    Ok(stdout.trim().to_string())
}

#[cfg(test)]
#[path = "../../tests/unit/infra/summary.rs"]
mod tests;
