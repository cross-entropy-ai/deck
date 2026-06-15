use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::worker::Worker;

const GITHUB_RELEASES_URL: &str =
    "https://api.github.com/repos/cross-entropy-ai/deck/releases/latest";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
pub const CACHE_TTL_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdateCheckMode {
    #[default]
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub latest_version: String,
    pub current_version: String,
    pub release_url: String,
    pub checked_at: u64,
}

pub enum UpdateRequest {
    Check,
}

pub enum UpdateResult {
    Ok {
        status: UpdateStatus,
        newer_than_current: bool,
    },
    Err(String),
}

/// Background GitHub release checker wrapping the generic [`Worker`] service:
/// `Check` requests run an HTTP probe and reply with an [`UpdateResult`];
/// dropping the checker ends the loop. Drop is **non-blocking** — `Worker`'s
/// drop signals and detaches rather than `join()`ing a possibly mid-HTTP
/// worker on the UI thread; an in-flight request just finishes unread.
pub struct UpdateChecker {
    worker: Worker<UpdateRequest, UpdateResult>,
}

impl UpdateChecker {
    pub fn spawn() -> Self {
        let worker = Worker::spawn_service("deck-update-check", |req, tx| match req {
            // Keep the loop alive as long as the UI keeps the channel open;
            // dropping `UpdateChecker` (its `Worker`) ends the `recv` and
            // the thread exits — no blocking join.
            UpdateRequest::Check => tx.send(do_check()).is_ok(),
        });
        UpdateChecker { worker }
    }

    pub fn request(&self, req: UpdateRequest) {
        self.worker.request(req);
    }

    pub fn try_recv(&self) -> Option<UpdateResult> {
        self.worker.try_recv()
    }
}

fn do_check() -> UpdateResult {
    let current = env!("CARGO_PKG_VERSION").to_string();
    match fetch_latest() {
        Ok((latest_version, release_url)) => {
            let newer = match compare(&current, &latest_version) {
                Some(b) => b,
                None => {
                    return UpdateResult::Err(format!(
                        "could not compare versions: current={} latest={}",
                        current, latest_version
                    ))
                }
            };
            UpdateResult::Ok {
                status: UpdateStatus {
                    latest_version,
                    current_version: current,
                    release_url,
                    checked_at: now_secs(),
                },
                newer_than_current: newer,
            }
        }
        Err(e) => UpdateResult::Err(e),
    }
}

fn fetch_latest() -> Result<(String, String), String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("deck/{}", env!("CARGO_PKG_VERSION")))
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("build client: {}", e))?;
    let resp = client
        .get(GITHUB_RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("request: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status));
    }
    let body = resp.text().map_err(|e| format!("read body: {}", e))?;
    crate::infra::parser::release::parse_release_json(&body)
}

/// Returns `Some(true)` iff `latest > current` under semver. `None` on parse failure.
pub fn compare(current: &str, latest: &str) -> Option<bool> {
    let cur = Version::parse(current).ok()?;
    let lat = Version::parse(latest).ok()?;
    Some(lat > cur)
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Render an elapsed duration (in seconds) as a compact "Xm ago" age:
/// "just now" under a minute, then `m` / `h` / `d` as it grows.
pub fn relative_age(elapsed_secs: u64) -> String {
    if elapsed_secs < 60 {
        "just now".to_string()
    } else if elapsed_secs < 3600 {
        format!("{}m ago", elapsed_secs / 60)
    } else if elapsed_secs < 86_400 {
        format!("{}h ago", elapsed_secs / 3600)
    } else {
        format!("{}d ago", elapsed_secs / 86_400)
    }
}

// --- Cache ---

fn cache_path() -> PathBuf {
    crate::config::config_dir_for("deck").join("update-cache.json")
}

pub struct UpdateCache;

impl UpdateCache {
    pub fn load() -> Option<UpdateStatus> {
        let content = fs::read_to_string(cache_path()).ok()?;
        serde_json::from_str(&content).ok()
    }

    pub fn save(status: &UpdateStatus) {
        let path = cache_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(status) {
            let _ = fs::write(&path, json);
        }
    }

    pub fn is_fresh(status: &UpdateStatus, now: u64, ttl: u64) -> bool {
        now.saturating_sub(status.checked_at) < ttl
    }
}

#[cfg(test)]
#[path = "../../tests/unit/infra/update.rs"]
mod tests;
