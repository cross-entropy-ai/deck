use std::sync::{Arc, Mutex};

use crate::app::port_forward_task::{Op, Runner, Worker};
use crate::config::{ForwardMode, ForwardSpec};

#[derive(Default, Clone)]
struct MockRunner {
    log: Arc<Mutex<Vec<String>>>,
    fail_master: Arc<Mutex<Vec<String>>>,
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
