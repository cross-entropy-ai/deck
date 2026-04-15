use std::process::Command;

/// Git info for a directory.
#[derive(Debug, Clone, Default)]
pub struct GitInfo {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
}

/// Get git branch and status for a directory.
pub fn get_git_info(dir: &str) -> GitInfo {
    if dir.is_empty() {
        return GitInfo::default();
    }

    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-b"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success());

    let Some(output) = output else {
        return GitInfo::default();
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut info = GitInfo::default();

    for line in text.lines() {
        if let Some(header) = line.strip_prefix("## ") {
            parse_branch_header(header, &mut info);
        } else if line.len() >= 2 {
            let bytes = line.as_bytes();
            let x = bytes[0];
            let y = bytes[1];

            if x == b'?' && y == b'?' {
                info.untracked += 1;
            } else {
                if x != b' ' && x != b'?' {
                    info.staged += 1;
                }
                if y != b' ' && y != b'?' {
                    info.modified += 1;
                }
            }
        }
    }

    info
}

fn parse_branch_header(header: &str, info: &mut GitInfo) {
    // Format: "branch...remote [ahead N, behind M]" or "branch...remote" or "branch"
    let branch_part = header.split("...").next().unwrap_or(header);
    info.branch = branch_part.to_string();

    if let Some(bracket_start) = header.find('[') {
        if let Some(bracket_end) = header.find(']') {
            let tracking = &header[bracket_start + 1..bracket_end];
            for part in tracking.split(", ") {
                if let Some(n) = part.strip_prefix("ahead ") {
                    info.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix("behind ") {
                    info.behind = n.parse().unwrap_or(0);
                }
            }
        }
    }
}
