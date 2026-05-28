use crate::config::{ForwardMode, ForwardSpec};
use crate::infra::port_forward::{
    build_cancel_cmd, build_exit_cmd, build_forward_cmd, build_master_cmd, ssh_args_for_host,
};

fn args_of(cmd: &std::process::Command) -> Vec<String> {
    cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
}

#[test]
fn ssh_args_uses_shared_control_options() {
    let args = ssh_args_for_host("server-1");
    let joined = args.join(" ");
    assert!(joined.contains("ControlMaster=auto"), "missing ControlMaster: {}", joined);
    assert!(joined.contains("ControlPath="), "missing ControlPath: {}", joined);
    assert!(joined.contains("ControlPersist="), "missing ControlPersist: {}", joined);
    assert!(args.contains(&"server-1".to_string()), "host missing from args");
}

#[test]
fn master_cmd_uses_fn_and_no_forwards() {
    let cmd = build_master_cmd("server-1");
    let args = args_of(&cmd);
    let joined = args.join(" ");
    assert!(joined.contains("-f"), "expected -f: {}", joined);
    assert!(joined.contains("-N"), "expected -N: {}", joined);
    assert!(!joined.contains(" -L "));
    assert!(!joined.contains(" -R "));
    assert!(!joined.contains(" -D "));
}

#[test]
fn forward_cmd_local() {
    let spec = ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: 8080,
        target_host: Some("localhost".into()),
        target_port: Some(80),
    };
    let cmd = build_forward_cmd("h", &spec);
    let args = args_of(&cmd);
    assert!(args.iter().any(|a| a == "-O"), "missing -O");
    let oi = args.iter().position(|a| a == "-O").unwrap();
    assert_eq!(args[oi + 1], "forward");
    assert!(args.contains(&"-L".into()));
    assert!(args.contains(&"8080:localhost:80".into()));
}

#[test]
fn cancel_cmd_uses_o_cancel() {
    let spec = ForwardSpec {
        mode: ForwardMode::Dynamic,
        bind_addr: None,
        listen_port: 1080,
        target_host: None,
        target_port: None,
    };
    let cmd = build_cancel_cmd("h", &spec);
    let args = args_of(&cmd);
    let oi = args.iter().position(|a| a == "-O").unwrap();
    assert_eq!(args[oi + 1], "cancel");
    assert!(args.contains(&"-D".into()));
    assert!(args.contains(&"1080".into()));
}

#[test]
fn exit_cmd_uses_o_exit() {
    let cmd = build_exit_cmd("h");
    let args = args_of(&cmd);
    let oi = args.iter().position(|a| a == "-O").unwrap();
    assert_eq!(args[oi + 1], "exit");
}
