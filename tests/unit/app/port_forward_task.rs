use std::sync::{Arc, Mutex};

use crate::app::ssh::port_forward_task::{Op, Runner, Worker};
use crate::forwards::{ForwardMode, ForwardSpec};

#[derive(Default, Clone)]
struct MockRunner {
    log: Arc<Mutex<Vec<String>>>,
    settings_log: Arc<Mutex<Vec<(String, crate::ssh::ConnectionSettings)>>>,
    fail_master: Arc<Mutex<Vec<String>>>,
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
        self.log
            .lock()
            .unwrap()
            .push(format!("forward {} {}", host, spec.listen_port));
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
        self.log
            .lock()
            .unwrap()
            .push(format!("cancel {} {}", host, spec.listen_port));
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
}

fn enabled_settings(path: &str, persist: &str) -> crate::ssh::ConnectionSettings {
    crate::ssh::ConnectionSettings {
        enabled: true,
        control_path: path.into(),
        control_persist: persist.into(),
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
        host: "h1".into(),
        spec: spec(8080),
    });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["master h1", "forward h1 8080"]);
    assert!(results.iter().all(|r| r.ok));
}

#[test]
fn add_forward_second_time_skips_master() {
    let runner = MockRunner::default();
    let mut w = Worker::new(runner.clone(), crate::ssh::ConnectionSettings::default());
    w.handle(Op::AddForward {
        host: "h1".into(),
        spec: spec(8080),
    });
    runner.log.lock().unwrap().clear();
    w.handle(Op::AddForward {
        host: "h1".into(),
        spec: spec(9090),
    });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["forward h1 9090"]);
}

#[test]
fn add_forward_master_failure_skips_forward() {
    let runner = MockRunner::default();
    runner.fail_master.lock().unwrap().push("h1".into());
    let mut w = Worker::new(runner.clone(), crate::ssh::ConnectionSettings::default());
    let results = w.handle(Op::AddForward {
        host: "h1".into(),
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
        host: "h1".into(),
        spec: spec(8080),
    });
    runner.log.lock().unwrap().clear();
    w.handle(Op::CancelForward {
        host: "h1".into(),
        spec: spec(8080),
    });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["cancel h1 8080"]);
}

#[test]
fn bootstrap_orders_master_before_each_host_forwards() {
    let runner = MockRunner::default();
    let mut w = Worker::new(runner.clone(), crate::ssh::ConnectionSettings::default());
    w.handle(Op::Bootstrap {
        hosts: vec![
            ("h1".into(), vec![spec(8080), spec(9090)]),
            ("h2".into(), vec![spec(7070)]),
        ],
    });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(
        log,
        vec![
            "master h1",
            "forward h1 8080",
            "forward h1 9090",
            "master h2",
            "forward h2 7070",
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
        stop_hosts: vec!["h1".into(), "h1".into(), "h2".into()],
        forward_hosts: vec![("h1".into(), vec![spec(8080)])],
    });

    assert_eq!(
        runner.log.lock().unwrap().as_slice(),
        ["exit h1", "exit h2"]
    );
    let snapshots = runner.settings_log.lock().unwrap();
    assert!(snapshots.iter().all(|(_, settings)| settings == &old));
}

#[test]
fn path_or_duration_change_closes_old_socket_then_restores_on_new_one() {
    let runner = MockRunner::default();
    let old = enabled_settings("/tmp/deck-old/cm-%C", "10m");
    let new = enabled_settings("/tmp/deck-new/cm-%C", "1h30m");
    let mut w = Worker::new(runner.clone(), old.clone());

    w.handle(Op::Reconfigure {
        settings: new.clone(),
        stop_hosts: vec!["h1".into()],
        forward_hosts: vec![("h1".into(), vec![spec(8080)])],
    });

    assert_eq!(
        runner.log.lock().unwrap().as_slice(),
        ["exit h1", "master h1", "forward h1 8080"]
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
            host: "h1".into(),
            spec: spec(8080),
        })
        .is_empty());
    assert!(w
        .handle(Op::CancelForward {
            host: "h1".into(),
            spec: spec(8080),
        })
        .is_empty());
    assert!(runner.log.lock().unwrap().is_empty());

    w.handle(Op::Reconfigure {
        settings: crate::ssh::ConnectionSettings::default(),
        stop_hosts: vec!["h1".into()],
        forward_hosts: vec![("h1".into(), vec![spec(8080)])],
    });
    assert_eq!(
        runner.log.lock().unwrap().as_slice(),
        ["master h1", "forward h1 8080"]
    );
}
