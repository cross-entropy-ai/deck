use crate::infra::listeners::{parse_netstat, parse_ss};

#[test]
fn netstat_extracts_listen_ports() {
    let sample = "\
Proto Recv-Q Send-Q  Local Address          Foreign Address        (state)
tcp4       0      0  127.0.0.1.8080         *.*                    LISTEN
tcp6       0      0  ::1.8080               *.*                    LISTEN
tcp46      0      0  *.1080                 *.*                    LISTEN
tcp4       0      0  127.0.0.1.52345        93.184.216.34.443      ESTABLISHED";
    let ports = parse_netstat(sample);
    assert!(ports.contains(&8080), "8080 should be LISTEN");
    assert!(ports.contains(&1080), "1080 should be LISTEN");
    assert!(!ports.contains(&52345), "ESTABLISHED row must be ignored");
    assert_eq!(
        ports.len(),
        2,
        "8080 appears twice (v4+v6), deduped, plus 1080"
    );
}

#[test]
fn ss_extracts_listen_ports() {
    let sample = "\
State      Recv-Q Send-Q Local Address:Port  Peer Address:Port
LISTEN     0      128    0.0.0.0:8080        0.0.0.0:*
LISTEN     0      128    [::]:8080           [::]:*
LISTEN     0      4096   127.0.0.1:1080      0.0.0.0:*
LISTEN     0      128    *:9090              *:*";
    let ports = parse_ss(sample);
    assert!(ports.contains(&8080));
    assert!(ports.contains(&1080));
    assert!(ports.contains(&9090));
    assert_eq!(ports.len(), 3);
}

#[test]
fn ss_handles_modern_netid_column() {
    let sample = "\
Netid State  Recv-Q Send-Q Local Address:Port Peer Address:Port
tcp   LISTEN 0      128    0.0.0.0:8080       0.0.0.0:*
tcp   LISTEN 0      4096   127.0.0.1:1080     0.0.0.0:*
tcp   LISTEN 0      128    [::]:9090          [::]:*";
    let ports = parse_ss(sample);
    assert!(ports.contains(&8080));
    assert!(ports.contains(&1080));
    assert!(ports.contains(&9090));
    assert_eq!(ports.len(), 3);
}

#[test]
fn ignores_header_and_empty() {
    assert!(parse_ss("State Recv-Q Send-Q Local Address:Port Peer Address:Port").is_empty());
    assert!(parse_netstat("Active Internet connections (including servers)").is_empty());
    assert!(parse_ss("").is_empty());
}
