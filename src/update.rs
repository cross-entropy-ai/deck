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
mod tests {
    use super::*;

    #[test]
    fn compare_newer_returns_true() {
        assert_eq!(compare("0.1.3", "0.2.0"), Some(true));
    }

    #[test]
    fn compare_equal_returns_false() {
        assert_eq!(compare("0.2.0", "0.2.0"), Some(false));
    }

    #[test]
    fn compare_older_returns_false() {
        assert_eq!(compare("0.3.0", "0.2.0"), Some(false));
    }

    #[test]
    fn compare_respects_numeric_order() {
        // Pure string compare would flip this: "0.10.0" < "0.9.9" lex.
        assert_eq!(compare("0.9.9", "0.10.0"), Some(true));
    }

    #[test]
    fn compare_stable_is_newer_than_prerelease() {
        assert_eq!(compare("0.2.0-beta.1", "0.2.0"), Some(true));
        assert_eq!(compare("0.2.0", "0.2.0-beta.1"), Some(false));
    }

    #[test]
    fn compare_garbage_returns_none() {
        assert_eq!(compare("garbage", "0.2.0"), None);
        assert_eq!(compare("0.2.0", "whatever"), None);
    }

    #[test]
    fn parse_release_strips_v_prefix() {
        let body = r#"{"tag_name":"v0.2.0","html_url":"https://example.com/tag"}"#;
        let (ver, url) = parse_release_json(body).unwrap();
        assert_eq!(ver, "0.2.0");
        assert_eq!(url, "https://example.com/tag");
    }

    #[test]
    fn parse_release_without_v_prefix_ok() {
        let body = r#"{"tag_name":"0.2.0","html_url":"https://example.com/tag"}"#;
        let (ver, _) = parse_release_json(body).unwrap();
        assert_eq!(ver, "0.2.0");
    }

    #[test]
    fn parse_release_missing_field_errors() {
        let body = r#"{"tag_name":"v0.2.0"}"#;
        assert!(parse_release_json(body).is_err());
    }

    #[test]
    fn parse_release_invalid_json_errors() {
        assert!(parse_release_json("not json").is_err());
    }

    #[test]
    fn cache_is_fresh_boundary() {
        let status = UpdateStatus {
            latest_version: "0.2.0".into(),
            current_version: "0.1.3".into(),
            release_url: String::new(),
            checked_at: 1000,
        };
        assert!(UpdateCache::is_fresh(&status, 1100, 200));
        assert!(UpdateCache::is_fresh(&status, 1199, 200));
        // Exactly ttl elapsed → stale.
        assert!(!UpdateCache::is_fresh(&status, 1200, 200));
        assert!(!UpdateCache::is_fresh(&status, 1201, 200));
    }

    #[test]
    fn cache_is_fresh_handles_clock_skew() {
        let status = UpdateStatus {
            latest_version: "0.2.0".into(),
            current_version: "0.1.3".into(),
            release_url: String::new(),
            checked_at: 2000,
        };
        // now < checked_at — clock moved backwards; treat as fresh.
        assert!(UpdateCache::is_fresh(&status, 1500, 200));
    }

    #[test]
    fn update_status_round_trip() {
        let status = UpdateStatus {
            latest_version: "0.2.0".into(),
            current_version: "0.1.3".into(),
            release_url: "https://example.com".into(),
            checked_at: 1234,
        };
        let json = serde_json::to_string(&status).unwrap();
        let parsed: UpdateStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, status);
    }

    #[test]
    fn update_check_mode_default_is_enabled() {
        assert_eq!(UpdateCheckMode::default(), UpdateCheckMode::Enabled);
    }

    #[test]
    fn update_check_mode_serialization() {
        assert_eq!(
            serde_json::to_string(&UpdateCheckMode::Enabled).unwrap(),
            "\"enabled\""
        );
        assert_eq!(
            serde_json::to_string(&UpdateCheckMode::Disabled).unwrap(),
            "\"disabled\""
        );
    }
}
