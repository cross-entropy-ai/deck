use std::sync::{Arc, Mutex};

use crate::app::ssh::port_forward_task::{
    ContainerEndpoint, ForwardEndpoint, MasterTarget, Op, Runner, Worker,
};
use crate::forwards::{ForwardMode, ForwardSpec};
use crate::system::tmux::TmuxSystem;

#[derive(Default, Clone)]
struct MockRunner {
    log: Arc<Mutex<Vec<String>>>,
    settings_log: Arc<Mutex<Vec<(String, crate::ssh::ConnectionSettings)>>>,
    fail_master: Arc<Mutex<Vec<String>>>,
    /// Where a container resolves to, or `None` to make resolution fail — the
    /// unreachable-container case the worker has to report as the forward
    /// failing rather than binding a listener to nothing.
    container_target: Arc<Mutex<Option<String>>>,
}

impl Runner for MockRunner {
    fn run_master(
        &self,
        settings: &crate::ssh::ConnectionSettings,
        host: &str,
    ) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("master {}", host));
        self.settings_log
            .lock()
            .unwrap()
            .push(("master".into(), settings.clone()));
        if self.fail_master.lock().unwrap().iter().any(|h| h == host) {
            Err("mock master failed".into())
        } else {
            Ok(())
        }
    }
    fn run_forward(
        &self,
        settings: &crate::ssh::ConnectionSettings,
        host: &str,
        spec: &ForwardSpec,
    ) -> Result<(), String> {
        self.log.lock().unwrap().push(format!(
            "forward {} {} -> {}:{}",
            host,
            spec.listen_port,
            spec.target_host.as_deref().unwrap_or(""),
            spec.target_port.unwrap_or(0)
        ));
        self.settings_log
            .lock()
            .unwrap()
            .push(("forward".into(), settings.clone()));
        Ok(())
    }
    fn run_cancel(
        &self,
        settings: &crate::ssh::ConnectionSettings,
        host: &str,
        spec: &ForwardSpec,
    ) -> Result<(), String> {
        self.log.lock().unwrap().push(format!(
            "cancel {} {} -> {}:{}",
            host,
            spec.listen_port,
            spec.target_host.as_deref().unwrap_or(""),
            spec.target_port.unwrap_or(0)
        ));
        self.settings_log
            .lock()
            .unwrap()
            .push(("cancel".into(), settings.clone()));
        Ok(())
    }
    fn run_exit(
        &self,
        settings: &crate::ssh::ConnectionSettings,
        host: &str,
    ) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("exit {}", host));
        self.settings_log
            .lock()
            .unwrap()
            .push(("exit".into(), settings.clone()));
        Ok(())
    }
    fn resolve_container_target(
        &self,
        host: &str,
        container: &ContainerEndpoint,
        port: u16,
    ) -> Result<String, String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("resolve {} {} {}", host, container.name, port));
        self.container_target
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "mock container unreachable".to_string())
    }
}

fn enabled_settings(path: &str, persist: &str) -> crate::ssh::ConnectionSettings {
    crate::ssh::ConnectionSettings {
        enabled: true,
        control_path: path.into(),
        control_persist: persist.into(),
    }
}

/// A host endpoint: the lane names itself, and the target is in the rule.
fn host_at(host: &str) -> ForwardEndpoint {
    ForwardEndpoint {
        lane: TmuxSystem::host_lane(host),
        host: host.to_string(),
        container: None,
    }
}

/// A container endpoint: reported to the container's lane, run over the host's
/// master, resolved through the engine.
fn container_at(host: &str, name: &str) -> ForwardEndpoint {
    ForwardEndpoint {
        lane: TmuxSystem::container_lane(host, name),
        host: host.to_string(),
        container: Some(ContainerEndpoint {
            engine: "docker".into(),
            name: name.into(),
        }),
    }
}

fn stop(host: &str) -> MasterTarget {
    MasterTarget {
        lane: TmuxSystem::host_lane(host),
        host: host.to_string(),
    }
}

/// A rule with no target address — what a container lane's form produces.
fn container_spec(listen: u16, inside: u16) -> ForwardSpec {
    ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: listen,
        target_host: None,
        target_port: Some(inside),
    }
}

fn spec(port: u16) -> ForwardSpec {
    ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: port,
        target_host: Some("h".into()),
        target_port: Some(80),
    }
}

#[test]
fn add_forward_starts_master_first_time() {
    let runner = MockRunner::default();
    let mut w = Worker::new(runner.clone(), crate::ssh::ConnectionSettings::default());
    let results = w.handle(Op::AddForward {
        endpoint: host_at("h1"),
        spec: spec(8080),
    });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["master h1", "forward h1 8080 -> h:80"]);
    assert!(results.iter().all(|r| r.ok));
}

#[test]
fn add_forward_second_time_skips_master() {
    let runner = MockRunner::default();
    let mut w = Worker::new(runner.clone(), crate::ssh::ConnectionSettings::default());
    w.handle(Op::AddForward {
        endpoint: host_at("h1"),
        spec: spec(8080),
    });
    runner.log.lock().unwrap().clear();
    w.handle(Op::AddForward {
        endpoint: host_at("h1"),
        spec: spec(9090),
    });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["forward h1 9090 -> h:80"]);
}

#[test]
fn add_forward_master_failure_skips_forward() {
    let runner = MockRunner::default();
    runner.fail_master.lock().unwrap().push("h1".into());
    let mut w = Worker::new(runner.clone(), crate::ssh::ConnectionSettings::default());
    let results = w.handle(Op::AddForward {
        endpoint: host_at("h1"),
        spec: spec(8080),
    });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["master h1"]);
    assert!(!results[0].ok);
}

#[test]
fn cancel_forward_does_not_touch_master() {
    let runner = MockRunner::default();
    let mut w = Worker::new(runner.clone(), crate::ssh::ConnectionSettings::default());
    w.handle(Op::AddForward {
        endpoint: host_at("h1"),
        spec: spec(8080),
    });
    runner.log.lock().unwrap().clear();
    w.handle(Op::CancelForward {
        endpoint: host_at("h1"),
        spec: spec(8080),
    });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["cancel h1 8080 -> h:80"]);
}

#[test]
fn bootstrap_orders_master_before_each_host_forwards() {
    let runner = MockRunner::default();
    let mut w = Worker::new(runner.clone(), crate::ssh::ConnectionSettings::default());
    w.handle(Op::Bootstrap {
        lanes: vec![
            (host_at("h1"), vec![spec(8080), spec(9090)]),
            (host_at("h2"), vec![spec(7070)]),
        ],
    });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(
        log,
        vec![
            "master h1",
            "forward h1 8080 -> h:80",
            "forward h1 9090 -> h:80",
            "master h2",
            "forward h2 7070 -> h:80",
        ]
    );
}

#[test]
fn disabling_exits_old_masters_and_does_not_restore_forwards() {
    let runner = MockRunner::default();
    let old = enabled_settings("/tmp/deck-old/cm-%C", "10m");
    let mut w = Worker::new(runner.clone(), old.clone());
    let mut disabled = old.clone();
    disabled.enabled = false;

    w.handle(Op::Reconfigure {
        settings: disabled,
        stop_hosts: vec![stop("h1"), stop("h1"), stop("h2")],
        forward_lanes: vec![(host_at("h1"), vec![spec(8080)])],
    });

    assert_eq!(
        runner.log.lock().unwrap().as_slice(),
        ["exit h1", "exit h2"]
    );
    let snapshots = runner.settings_log.lock().unwrap();
    assert!(snapshots.iter().all(|(_, settings)| settings == &old));
}

#[test]
fn duration_only_change_leaves_live_masters_and_forwards_untouched() {
    // `ssh -O exit` would kill the multiplexed `tmux attach` PTYs riding on the
    // master, and the socket location hasn't moved — so this must be a no-op.
    let runner = MockRunner::default();
    let old = enabled_settings("/tmp/deck-old/cm-%C", "10m");
    let mut w = Worker::new(runner.clone(), old.clone());

    let results = w.handle(Op::Reconfigure {
        settings: enabled_settings("/tmp/deck-old/cm-%C", "1h30m"),
        stop_hosts: vec![stop("h1"), stop("h2")],
        forward_lanes: vec![(host_at("h1"), vec![spec(8080)])],
    });

    assert!(results.is_empty());
    assert!(
        runner.log.lock().unwrap().is_empty(),
        "no ssh command should run: {:?}",
        runner.log.lock().unwrap()
    );
}

#[test]
fn path_change_closes_old_socket_then_restores_on_new_one() {
    let runner = MockRunner::default();
    let old = enabled_settings("/tmp/deck-old/cm-%C", "10m");
    let new = enabled_settings("/tmp/deck-new/cm-%C", "1h30m");
    let mut w = Worker::new(runner.clone(), old.clone());

    w.handle(Op::Reconfigure {
        settings: new.clone(),
        stop_hosts: vec![stop("h1")],
        forward_lanes: vec![(host_at("h1"), vec![spec(8080)])],
    });

    assert_eq!(
        runner.log.lock().unwrap().as_slice(),
        ["exit h1", "master h1", "forward h1 8080 -> h:80"]
    );
    let snapshots = runner.settings_log.lock().unwrap();
    assert_eq!(snapshots[0], ("exit".into(), old));
    assert_eq!(snapshots[1], ("master".into(), new.clone()));
    assert_eq!(snapshots[2], ("forward".into(), new));
}

#[test]
fn disabled_worker_rejects_forward_mutations_until_reenabled() {
    let runner = MockRunner::default();
    let disabled = crate::ssh::ConnectionSettings {
        enabled: false,
        ..crate::ssh::ConnectionSettings::default()
    };
    let mut w = Worker::new(runner.clone(), disabled);

    assert!(w
        .handle(Op::AddForward {
            endpoint: host_at("h1"),
            spec: spec(8080),
        })
        .is_empty());
    assert!(w
        .handle(Op::CancelForward {
            endpoint: host_at("h1"),
            spec: spec(8080),
        })
        .is_empty());
    assert!(runner.log.lock().unwrap().is_empty());

    w.handle(Op::Reconfigure {
        settings: crate::ssh::ConnectionSettings::default(),
        stop_hosts: vec![stop("h1")],
        forward_lanes: vec![(host_at("h1"), vec![spec(8080)])],
    });
    assert_eq!(
        runner.log.lock().unwrap().as_slice(),
        ["master h1", "forward h1 8080 -> h:80"]
    );
}

#[test]
fn a_container_forward_points_at_the_address_the_engine_reports() {
    let runner = MockRunner::default();
    *runner.container_target.lock().unwrap() = Some("172.17.0.2:8080".into());
    let mut w = Worker::new(runner.clone(), crate::ssh::ConnectionSettings::default());

    let results = w.handle(Op::AddForward {
        endpoint: container_at("box", "dev"),
        spec: container_spec(9000, 8080),
    });

    // The master is the *host's* — a container id is not an ssh destination —
    // and the rule the user stored carries no address, so one is resolved.
    assert_eq!(
        runner.log.lock().unwrap().as_slice(),
        [
            "master box",
            "resolve box dev 8080",
            "forward box 9000 -> 172.17.0.2:8080",
        ]
    );
    // Reported to the container's lane, not its host's: that is whose overlay
    // is open and whose config the rule is saved into.
    assert!(results.iter().all(|r| r.ok));
    assert_eq!(
        results.last().unwrap().kind.lane(),
        &TmuxSystem::container_lane("box", "dev")
    );
}

#[test]
fn cancelling_a_container_forward_names_the_address_it_was_added_with() {
    // A container's address is resolved fresh on every apply, so re-resolving
    // at cancel time would name a *different* endpoint the moment the container
    // restarted — and the listener would stay bound with nothing able to close
    // it. What was added is what gets cancelled.
    let runner = MockRunner::default();
    *runner.container_target.lock().unwrap() = Some("172.17.0.2:8080".into());
    let mut w = Worker::new(runner.clone(), crate::ssh::ConnectionSettings::default());
    w.handle(Op::AddForward {
        endpoint: container_at("box", "dev"),
        spec: container_spec(9000, 8080),
    });

    // The container comes back on a different address.
    *runner.container_target.lock().unwrap() = Some("172.17.0.9:8080".into());
    runner.log.lock().unwrap().clear();
    w.handle(Op::CancelForward {
        endpoint: container_at("box", "dev"),
        spec: container_spec(9000, 8080),
    });

    assert_eq!(
        runner.log.lock().unwrap().as_slice(),
        ["cancel box 9000 -> 172.17.0.2:8080"],
        "the remembered address, and no second resolve"
    );
}

#[test]
fn a_container_deck_cannot_reach_fails_the_forward_instead_of_binding() {
    // `ssh -O forward` reports success as soon as the *local* listener binds,
    // so an unresolvable endpoint must stop before that: otherwise the rule
    // looks applied, is saved, and only surfaces as connections that hang.
    let runner = MockRunner::default();
    *runner.container_target.lock().unwrap() = None;
    let mut w = Worker::new(runner.clone(), crate::ssh::ConnectionSettings::default());

    let results = w.handle(Op::AddForward {
        endpoint: container_at("box", "dev"),
        spec: container_spec(9000, 8080),
    });

    assert_eq!(
        runner.log.lock().unwrap().as_slice(),
        ["master box", "resolve box dev 8080"],
        "no forward should be attempted"
    );
    let last = results.last().unwrap();
    assert!(!last.ok);
    assert!(last.message.contains("unreachable"), "{}", last.message);
    assert_eq!(
        last.kind.lane(),
        &TmuxSystem::container_lane("box", "dev"),
        "the failure belongs to the container's overlay"
    );
}

#[test]
fn a_host_and_its_container_share_one_master() {
    let runner = MockRunner::default();
    *runner.container_target.lock().unwrap() = Some("172.17.0.2:8080".into());
    let mut w = Worker::new(runner.clone(), crate::ssh::ConnectionSettings::default());

    w.handle(Op::Bootstrap {
        lanes: vec![
            (host_at("box"), vec![spec(8080)]),
            (container_at("box", "dev"), vec![container_spec(9000, 8080)]),
        ],
    });

    assert_eq!(
        runner.log.lock().unwrap().as_slice(),
        [
            "master box",
            "forward box 8080 -> h:80",
            "resolve box dev 8080",
            "forward box 9000 -> 172.17.0.2:8080",
        ],
        "the container rides the master its host already brought up"
    );
}
