use crate::forwards::{diff_forwards, ForwardMode, ForwardOp, ForwardSpec};

#[test]
fn forward_spec_local_to_flag_no_bind() {
    let spec = ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: 8080,
        target_host: Some("example.com".into()),
        target_port: Some(80),
    };
    assert_eq!(spec.to_ssh_flag(), "-L 8080:example.com:80");
}

#[test]
fn forward_spec_remote_to_flag() {
    let spec = ForwardSpec {
        mode: ForwardMode::Remote,
        bind_addr: Some("0.0.0.0".into()),
        listen_port: 9090,
        target_host: Some("localhost".into()),
        target_port: Some(5432),
    };
    assert_eq!(spec.to_ssh_flag(), "-R 0.0.0.0:9090:localhost:5432");
}

#[test]
fn forward_spec_dynamic_to_flag() {
    let spec = ForwardSpec {
        mode: ForwardMode::Dynamic,
        bind_addr: None,
        listen_port: 1080,
        target_host: None,
        target_port: None,
    };
    assert_eq!(spec.to_ssh_flag(), "-D 1080");
}

#[test]
fn forward_spec_dynamic_with_bind_to_flag() {
    let spec = ForwardSpec {
        mode: ForwardMode::Dynamic,
        bind_addr: Some("127.0.0.1".into()),
        listen_port: 1080,
        target_host: None,
        target_port: None,
    };
    assert_eq!(spec.to_ssh_flag(), "-D 127.0.0.1:1080");
}

fn fwd(port: u16) -> ForwardSpec {
    ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: port,
        target_host: Some("localhost".into()),
        target_port: Some(80),
    }
}

#[test]
fn diff_forwards_unchanged_emits_nothing() {
    let v = vec![fwd(8080)];
    let ops = diff_forwards(&v, &v);
    assert!(ops.is_empty());
}

#[test]
fn diff_forwards_mixed() {
    let old = vec![fwd(8080), fwd(9090)];
    let new = vec![fwd(8080), fwd(7070)];
    let ops = diff_forwards(&old, &new);
    assert_eq!(ops.len(), 2);
    assert!(ops
        .iter()
        .any(|o| matches!(o, ForwardOp::Cancel(s) if s.listen_port == 9090)));
    assert!(ops
        .iter()
        .any(|o| matches!(o, ForwardOp::Add(s) if s.listen_port == 7070)));
}
