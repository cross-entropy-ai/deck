//! Parser for `~/.ssh/config` host aliases. The file read lives in
//! `infra::ssh`; this extracts the concrete `Host` aliases for the picker.

/// Parse `~/.ssh/config` text into the list of concrete `Host` aliases.
/// Each `Host` line may list several patterns; we keep those without
/// wildcard/negation characters (`*`, `?`, `!`), de-duped, first-seen order.
/// Effective per-host options are irrelevant here — the picker only needs the
/// alias to add to deck (later resolved via `ssh -G`).
pub fn parse_config_hosts(content: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let keyword = tokens.next().unwrap_or("");
        if !keyword.eq_ignore_ascii_case("host") {
            continue;
        }
        for pat in tokens {
            if pat.starts_with('#') {
                break; // rest of the line is an inline comment
            }
            if pat.contains('*') || pat.contains('?') || pat.contains('!') {
                continue;
            }
            if !hosts.iter().any(|h| h == pat) {
                hosts.push(pat.to_string());
            }
        }
    }
    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_concrete_hosts_excluding_wildcards() {
        let sample = "\
# work hosts
Host prod-web-1
    HostName 10.0.0.1
    User deploy

Host prod-web-2 staging
    HostName 10.0.0.2

Host *
    ServerAliveInterval 30

Host build-?
    HostName builder

Host !secret
    HostName x
";
        let hosts = parse_config_hosts(sample);
        assert_eq!(hosts, vec!["prod-web-1", "prod-web-2", "staging"]);
    }

    #[test]
    fn dedups_and_preserves_first_seen_order() {
        let sample = "Host a\nHost b\nHost a\n";
        assert_eq!(parse_config_hosts(sample), vec!["a", "b"]);
    }

    #[test]
    fn empty_or_no_hosts_yields_empty() {
        assert!(parse_config_hosts("").is_empty());
        assert!(parse_config_hosts("# comment only\n  IdentityFile ~/x\n").is_empty());
    }

    #[test]
    fn host_line_inline_comment_is_ignored() {
        let sample = "Host web # primary box\nHost db\n";
        assert_eq!(parse_config_hosts(sample), vec!["web", "db"]);
    }
}
