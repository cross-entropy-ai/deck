use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use semver::Version;
use serde::{Deserialize, Serialize};

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
    Shutdown,
}

pub enum UpdateResult {
    Ok {
        status: UpdateStatus,
        newer_than_current: bool,
    },
    Err(String),
}

pub struct UpdateChecker {
    tx: Sender<UpdateRequest>,
    rx: Receiver<UpdateResult>,
    handle: Option<JoinHandle<()>>,
}

impl UpdateChecker {
    pub fn spawn() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<UpdateRequest>();
        let (res_tx, res_rx) = mpsc::channel::<UpdateResult>();
        let handle = thread::spawn(move || worker_loop(req_rx, res_tx));
        UpdateChecker {
            tx: req_tx,
            rx: res_rx,
            handle: Some(handle),
        }
    }

    pub fn request(&self, req: UpdateRequest) {
        let _ = self.tx.send(req);
    }

    pub fn try_recv(&self) -> Option<UpdateResult> {
        self.rx.try_recv().ok()
    }
}

impl Drop for UpdateChecker {
    fn drop(&mut self) {
        let _ = self.tx.send(UpdateRequest::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn worker_loop(rx: Receiver<UpdateRequest>, tx: Sender<UpdateResult>) {
    loop {
        match rx.recv() {
            Ok(UpdateRequest::Check) => {
                let result = do_check();
                if tx.send(result).is_err() {
                    return;
                }
            }
            Ok(UpdateRequest::Shutdown) | Err(_) => return,
        }
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
    parse_release_json(&body)
}

pub fn parse_release_json(body: &str) -> Result<(String, String), String> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        html_url: String,
    }
    let r: Release = serde_json::from_str(body).map_err(|e| format!("parse: {}", e))?;
    let version = r.tag_name.trim_start_matches('v').to_string();
    Ok((version, r.html_url))
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

// --- Cache ---

fn cache_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("deck")
        .join("update-cache.json")
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

// --- Brew detection ---

pub fn has_brew() -> bool {
    Command::new("which")
        .arg("brew")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "../../tests/unit/infra/update.rs"]
mod tests;
