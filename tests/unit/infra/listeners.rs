use crate::infra::parser::listeners::{parse_lsof, parse_ss};

#[test]
fn lsof_extracts_listen_ports() {
    // `lsof -nP -iTCP -sTCP:LISTEN` — IPv4 wildcard, IPv6 bracketed, plain
    // loopback, with an ESTABLISHED row that must be ignored (no `(LISTEN)`).
    let sample = "\
COMMAND     PID  USER   FD   TYPE             DEVICE SIZE/OFF NODE NAME
sshd      19875 junyi    4u  IPv4 0xc6250bfe5691057      0t0  TCP *:8080 (LISTEN)
sshd      19875 junyi    5u  IPv6 0x20cfc4123dca9e46     0t0  TCP [::1]:8080 (LISTEN)
sshd      19875 junyi    6u  IPv4 0xde3646cbde046491     0t0  TCP 127.0.0.1:1080 (LISTEN)
Google    12345 junyi   30u  IPv4 0xaaaa                 0t0  TCP 127.0.0.1:52345->93.184.216.34:443 (ESTABLISHED)";
    let ports = parse_lsof(sample);
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
    assert!(parse_lsof("COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME").is_empty());
    assert!(parse_lsof("").is_empty());
    assert!(parse_ss("").is_empty());
}
