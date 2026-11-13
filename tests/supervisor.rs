//! The worker supervisor against a real process over real pipes, with no window.
//!
//! `tests/workers/fake_worker.py` misbehaves on demand (never ready, wrong protocol, garbage,
//! crashes, ignores EOF). `tests/real_workers.rs` runs shell-php's echo worker and Grafida's
//! `bin/worker.php` when those clones are present.

mod common;

use std::time::{Duration, Instant};

use common::{Harness, fake};
use grafida_shell::supervisor::Notice;
use serde_json::Value;

#[test]
fn requests_are_answered_with_repeated_headers_and_bodies_intact() {
    let h = Harness::start(fake("echo"));
    assert!(matches!(h.notice(), Notice::Ready { first: true, .. }));

    for size in [0usize, 1, 1023, 1024, 1025, 65_536, 1_048_577] {
        let body: Vec<u8> = (0..size).map(|i| (i * 31 % 251) as u8).collect();
        let r = h.request(
            "POST",
            "shell://app/api/echo?x=%C3%A9",
            &[("x-a", "1"), ("x-a", "2")],
            body.clone(),
        );
        assert_eq!(r.status, 200);
        assert_eq!(r.headers["x-multi"], vec!["one", "two"]);
        let json: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(json["bodyLength"], size);
        assert_eq!(json["bodySha256"], common::sha256(&body));
        assert_eq!(json["headers"]["x-a"], serde_json::json!(["1", "2"]));
        assert_eq!(json["url"], "shell://app/api/echo?x=%C3%A9");
        assert_eq!(json["maxBody"], (128u64 << 20).to_string());
    }
}

#[test]
fn a_crash_answers_the_pending_request_with_502_and_the_worker_comes_back() {
    let h = Harness::start(fake("echo"));
    h.notice();

    let before = h.pid();
    let r = h.request("GET", "shell://app/crash", &[], vec![]);
    assert_eq!(r.status, 502);
    assert_eq!(
        serde_json::from_slice::<Value>(&r.body).unwrap()["error"],
        "worker_restarted"
    );
    assert!(matches!(h.notice(), Notice::Restarting { .. }));
    assert!(matches!(h.notice(), Notice::Ready { first: false, .. }));

    assert_ne!(h.pid(), before, "a fresh worker answers");
}

#[test]
fn an_external_kill_is_a_crash_too() {
    let h = Harness::start(fake("echo"));
    h.notice();
    let pid = h.pid();
    common::kill(pid);
    assert!(matches!(h.notice(), Notice::Restarting { .. }));
    assert!(matches!(h.notice(), Notice::Ready { first: false, .. }));
    assert_ne!(h.pid(), pid);
}

#[test]
fn requests_queued_during_a_restart_are_served_by_the_new_worker() {
    let h = Harness::start(fake("echo"));
    h.notice();
    let crash = h.submit("GET", "shell://app/crash");
    let after = h.submit("GET", "shell://app/after");
    assert_eq!(
        crash.recv_timeout(Duration::from_secs(10)).unwrap().status,
        502
    );
    assert_eq!(
        after.recv_timeout(Duration::from_secs(10)).unwrap().status,
        200
    );
}

#[test]
fn one_crash_too_many_in_the_window_is_fatal_with_the_stderr_tail() {
    let h = Harness::start(fake("echo"));
    h.notice();
    h.request("GET", "shell://app/stderr?lines=45", &[], vec![]);

    for _ in 0..3 {
        assert_eq!(
            h.request("GET", "shell://app/crash", &[], vec![]).status,
            502
        );
        assert!(matches!(h.notice(), Notice::Restarting { .. }));
        assert!(matches!(h.notice(), Notice::Ready { .. }));
    }
    assert_eq!(
        h.request("GET", "shell://app/crash", &[], vec![]).status,
        502
    );

    let Notice::Fatal(failure) = h.notice() else {
        panic!("expected the supervisor to give up");
    };
    assert!(
        failure.reason.contains("4 times within 60 s"),
        "{}",
        failure.reason
    );
    assert_eq!(failure.stderr_tail.len(), 40);
    assert!(
        failure.stderr_tail.iter().any(|l| l == "line 45"),
        "{:?}",
        failure.stderr_tail
    );

    // From now on nothing reaches a worker.
    assert_eq!(h.request("GET", "shell://app/x", &[], vec![]).status, 503);
}

#[test]
fn a_worker_that_never_sends_ready_is_fatal_after_the_timeout() {
    let started = Instant::now();
    let h = Harness::start_with(fake("silent"), |c| {
        c.ready_timeout = Duration::from_millis(1500)
    });
    let Notice::Fatal(failure) = h.notice() else {
        panic!("expected a fatal notice");
    };
    assert!(
        failure.reason.contains("did not send `ready`"),
        "{}",
        failure.reason
    );
    assert!(started.elapsed() >= Duration::from_millis(1500));
    assert!(started.elapsed() < Duration::from_secs(6));
    // The worker it killed must not come back as a second crash and a second alert.
    h.assert_no_notice_for(Duration::from_millis(800));
}

#[test]
fn a_protocol_mismatch_is_fatal() {
    let h = Harness::start(fake("version"));
    let Notice::Fatal(failure) = h.notice() else {
        panic!("expected a fatal notice");
    };
    assert!(failure.reason.contains("protocol 2"), "{}", failure.reason);
}

#[test]
fn output_before_ready_is_a_protocol_error() {
    let h = Harness::start(fake("garbage"));
    let Notice::Fatal(failure) = h.notice() else {
        panic!("expected a fatal notice");
    };
    assert!(failure.reason.contains("bad_magic"), "{}", failure.reason);
}

#[test]
fn a_worker_that_dies_at_startup_is_fatal_with_its_stderr() {
    let h = Harness::start(fake("die"));
    let Notice::Fatal(failure) = h.notice() else {
        panic!("expected a fatal notice");
    };
    assert!(
        failure.reason.contains("exit status 3"),
        "{}",
        failure.reason
    );
    assert_eq!(
        failure.stderr_tail.last().map(String::as_str),
        Some("fatal line 50")
    );
}

#[test]
fn a_missing_worker_binary_is_fatal() {
    let mut command = fake("echo");
    command.program = "/nonexistent/worker".into();
    let h = Harness::start(command);
    let Notice::Fatal(failure) = h.notice() else {
        panic!("expected a fatal notice");
    };
    assert!(
        failure.reason.contains("cannot start the worker"),
        "{}",
        failure.reason
    );
}

#[test]
fn shutdown_lets_a_worker_exit_on_eof() {
    let h = Harness::start(fake("echo"));
    h.notice();
    let pid = h.pid();
    let started = Instant::now();
    h.shutdown();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!common::alive(pid));
}

#[test]
fn shutdown_kills_a_worker_that_ignores_eof_after_the_grace_period() {
    let h = Harness::start_with(fake("linger"), |c| {
        c.shutdown_grace = Duration::from_millis(500)
    });
    h.notice();
    let pid = h.pid();
    let started = Instant::now();
    h.shutdown();
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(500) && took < Duration::from_secs(3),
        "{took:?}"
    );
    assert!(!common::alive(pid));
}

#[test]
fn a_slow_request_does_not_block_submission() {
    let h = Harness::start(fake("echo"));
    h.notice();
    let started = Instant::now();
    let slow = h.submit("GET", "shell://app/sleep?ms=800");
    let next = h.submit("GET", "shell://app/next");
    // Both submits returned at once; the worker serves them in order.
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(
        slow.recv_timeout(Duration::from_secs(5)).unwrap().status,
        200
    );
    assert_eq!(
        next.recv_timeout(Duration::from_secs(5)).unwrap().status,
        200
    );
    assert!(started.elapsed() >= Duration::from_millis(800));
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn the_worker_dies_with_the_shell() {
    // A child shell process (this test binary, re-run as a stub) starts a worker that ignores EOF
    // and is then killed outright; PR_SET_PDEATHSIG (Linux) or the Job Object's
    // KILL_ON_JOB_CLOSE (Windows, grafida/grafida#91) must take the worker down with it.
    let pid = common::orphan_check("deaf", None);
    assert!(
        common::exits_within(pid, Duration::from_secs(5)),
        "worker {pid} outlived its shell"
    );
}

#[test]
fn an_idle_worker_exits_at_once_when_its_shell_is_killed() {
    // Everywhere, and the only mechanism on macOS (grafida/grafida#88): the shell's death closes
    // the worker's stdin, and a worker waiting for its next request reads EOF and exits.
    let pid = common::orphan_check("echo", None);
    assert!(
        common::exits_within(pid, Duration::from_secs(3)),
        "worker {pid} outlived its shell"
    );
}

#[test]
fn a_busy_worker_exits_when_its_request_ends_after_its_shell_is_killed() {
    // A worker in the middle of a request cannot read EOF until the request is over; then its
    // reply hits a closed pipe (or its next read EOF) and it exits. That bound is the design on
    // macOS, where nothing else ends it (grafida/grafida#88). On Linux PDEATHSIG is quicker, and
    // on Windows the Job Object kills it at once.
    let pid = common::orphan_check("echo", Some(3000));
    if cfg!(target_os = "macos") {
        std::thread::sleep(Duration::from_millis(1000));
        assert!(
            common::alive(pid),
            "worker {pid} was expected to finish its request first"
        );
    }
    assert!(
        common::exits_within(pid, Duration::from_secs(6)),
        "worker {pid} outlived its shell and its request"
    );
}

/// Not a test of its own: the stand-in shell `the_worker_dies_with_the_shell` runs.
#[test]
fn orphan_stub() {
    common::orphan_stub();
}
