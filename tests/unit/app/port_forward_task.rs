use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::app::port_forward_task::{Op, OpKind, Runner, Worker};
use crate::config::{ForwardMode, ForwardSpec};
use crate::state::{ForwardHealth, ForwardKey};

#[derive(Default, Clone)]
struct MockRunner {
    log: Arc<Mutex<Vec<String>>>,
    fail_master: Arc<Mutex<Vec<String>>>,
    listening: Arc<Mutex<Option<HashSet<u16>>>>,
}

impl Runner for MockRunner {
    fn run_master(&self, host: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("master {}", host));
        if self.fail_master.lock().unwrap().iter().any(|h| h == host) {
            Err("mock master failed".into())
        } else {
            Ok(())
        }
    }
    fn run_forward(&self, host: &str, spec: &ForwardSpec) -> Result<(), String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("forward {} {}", host, spec.listen_port));
        Ok(())
    }
    fn run_cancel(&self, host: &str, spec: &ForwardSpec) -> Result<(), String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("cancel {} {}", host, spec.listen_port));
        Ok(())
    }
    fn run_exit(&self, host: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("exit {}", host));
        Ok(())
    }
    fn listening_ports(&self) -> Option<HashSet<u16>> {
        self.listening.lock().unwrap().clone()
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
    let mut w = Worker::new(runner.clone());
    let results = w.handle(Op::AddForward { host: "h1".into(), spec: spec(8080) });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["master h1", "forward h1 8080"]);
    assert!(results.iter().all(|r| r.ok));
}

#[test]
fn add_forward_second_time_skips_master() {
    let runner = MockRunner::default();
    let mut w = Worker::new(runner.clone());
    w.handle(Op::AddForward { host: "h1".into(), spec: spec(8080) });
    runner.log.lock().unwrap().clear();
    w.handle(Op::AddForward { host: "h1".into(), spec: spec(9090) });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["forward h1 9090"]);
}

#[test]
fn add_forward_master_failure_skips_forward() {
    let runner = MockRunner::default();
    runner.fail_master.lock().unwrap().push("h1".into());
    let mut w = Worker::new(runner.clone());
    let results = w.handle(Op::AddForward { host: "h1".into(), spec: spec(8080) });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["master h1"]);
    assert!(!results[0].ok);
}

#[test]
fn cancel_forward_does_not_touch_master() {
    let runner = MockRunner::default();
    let mut w = Worker::new(runner.clone());
    w.handle(Op::AddForward { host: "h1".into(), spec: spec(8080) });
    runner.log.lock().unwrap().clear();
    w.handle(Op::CancelForward { host: "h1".into(), spec: spec(8080) });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["cancel h1 8080"]);
}

#[test]
fn bootstrap_orders_master_before_each_host_forwards() {
    let runner = MockRunner::default();
    let mut w = Worker::new(runner.clone());
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
fn probe_classifies_local_by_listeners_and_skips_remote() {
    let runner = MockRunner::default();
    *runner.listening.lock().unwrap() = Some(HashSet::from([8080u16])); // 8080 up, 1080 down
    let mut w = Worker::new(runner);

    let key = |mode, port| ForwardKey {
        host: "h".into(),
        mode,
        bind_addr: None,
        listen_port: port,
    };
    let results = w.handle(Op::Probe {
        items: vec![
            key(ForwardMode::Local, 8080),
            key(ForwardMode::Dynamic, 1080),
            // -R is filtered out: its liveness mirrors host reachability and
            // is derived in the app layer, never by the worker.
            key(ForwardMode::Remote, 9090),
        ],
    });

    // Only the two local-listener forwards come back; -R produces no result.
    assert_eq!(results.len(), 2, "worker must skip -R items");
    let health = |i: usize| match &results[i].kind {
        OpKind::Probe(_, h) => *h,
        other => panic!("expected Probe kind, got {:?}", other),
    };
    assert_eq!(health(0), ForwardHealth::Up); // -L 8080 is listening
    assert_eq!(health(1), ForwardHealth::Down); // -D 1080 not listening
    assert!(
        results.iter().all(|r| !matches!(
            &r.kind,
            OpKind::Probe(k, _) if matches!(k.mode, ForwardMode::Remote)
        )),
        "no -R result should be emitted"
    );
}

#[test]
fn probe_local_down_when_enumeration_unavailable() {
    let runner = MockRunner::default(); // listening = None
    let mut w = Worker::new(runner);
    let results = w.handle(Op::Probe {
        items: vec![ForwardKey {
            host: "h".into(),
            mode: ForwardMode::Local,
            bind_addr: None,
            listen_port: 8080,
        }],
    });
    match &results[0].kind {
        OpKind::Probe(_, h) => assert_eq!(*h, ForwardHealth::Probing),
        other => panic!("expected Probe kind, got {:?}", other),
    }
}
