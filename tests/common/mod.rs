//! Shared helpers for the supervisor tests.
#![allow(dead_code)]

use std::{
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use grafida_shell::{
    http::{self, Request, Response},
    protocol::HeaderMap,
    supervisor::{Config, Notice, Supervisor, WorkerCommand},
};
use sha2::{Digest, Sha256};

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The fake worker in the given mode.
pub fn fake(mode: &str) -> WorkerCommand {
    WorkerCommand {
        program: "python3".into(),
        args: vec![
            "-I".into(),
            manifest_dir().join("tests/workers/fake_worker.py").into(),
            mode.into(),
        ],
        cwd: manifest_dir(),
        env: Vec::new(),
    }
}

pub fn config(command: WorkerCommand) -> Config {
    Config {
        command,
        ready_timeout: Duration::from_secs(15),
        max_body_bytes: 128 << 20,
        restart_max: 3,
        restart_window: Duration::from_secs(60),
        shutdown_grace: Duration::from_secs(3),
    }
}

pub struct Harness {
    pub supervisor: Supervisor,
    notices: Receiver<Notice>,
    pub log: Receiver<(String, String)>,
}

impl Harness {
    pub fn start(command: WorkerCommand) -> Harness {
        Harness::start_with(command, |_| {})
    }

    pub fn start_with(command: WorkerCommand, tweak: impl FnOnce(&mut Config)) -> Harness {
        let mut config = config(command);
        tweak(&mut config);
        let (notice_tx, notices) = mpsc::channel();
        let (log_tx, log) = mpsc::channel();
        let log_tx = std::sync::Mutex::new(log_tx);
        let supervisor = Supervisor::start(
            config,
            Box::new(move |n| {
                let _ = notice_tx.send(n);
            }),
            std::sync::Arc::new(move |source: &str, line: &str| {
                let _ = log_tx.lock().unwrap().send((source.into(), line.into()));
            }),
        )
        .unwrap();
        Harness {
            supervisor,
            notices,
            log,
        }
    }

    pub fn notice(&self) -> Notice {
        self.notices
            .recv_timeout(Duration::from_secs(20))
            .expect("a supervisor notice")
    }

    /// Fails if the supervisor says anything more within `quiet`.
    pub fn assert_no_notice_for(&self, quiet: Duration) {
        if let Ok(extra) = self.notices.recv_timeout(quiet) {
            panic!("unexpected extra notice: {extra:?}");
        }
    }

    pub fn submit(&self, method: &str, url: &str) -> Receiver<Response> {
        self.submit_full(method, url, &[], Vec::new())
    }

    pub fn submit_full(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Receiver<Response> {
        let mut map = HeaderMap::new();
        for (k, v) in headers {
            http::append(&mut map, k, *v);
        }
        let (tx, rx) = mpsc::channel();
        self.supervisor.submit(
            Request {
                method: method.into(),
                url: url.into(),
                headers: map,
                body,
            },
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        rx
    }

    pub fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Response {
        self.submit_full(method, url, headers, body)
            .recv_timeout(Duration::from_secs(30))
            .expect("a response")
    }

    /// The pid of the worker currently answering (fake worker only).
    pub fn pid(&self) -> u32 {
        let r = self.request("GET", "boson://app/pid", &[], Vec::new());
        let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        v["pid"].as_u64().unwrap() as u32
    }

    pub fn shutdown(self) {
        self.supervisor.shutdown();
    }
}

pub fn alive(pid: u32) -> bool {
    // A zombie still answers kill(0), so look at the process state instead: /proc on Linux, ps
    // elsewhere (macOS has no /proc).
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => !stat
                .rsplit(')')
                .next()
                .is_some_and(|rest| rest.trim_start().starts_with('Z')),
            Err(_) => false,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let Ok(out) = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
        else {
            return false;
        };
        let state = String::from_utf8_lossy(&out.stdout);
        let state = state.trim();
        !state.is_empty() && !state.starts_with('Z')
    }
}

const STUB_ENV: &str = "SHELL_TEST_ORPHAN_STUB";
const STUB_MODE_ENV: &str = "SHELL_TEST_ORPHAN_MODE";
const STUB_BUSY_ENV: &str = "SHELL_TEST_ORPHAN_BUSY_MS";

/// Runs this test binary's `orphan_stub` as a stand-in shell with a fake worker in `mode`, kills
/// it with SIGKILL once its worker is ready (and, with `busy_ms`, once a `/sleep?ms=busy_ms`
/// request is in flight), and returns the worker's pid.
pub fn orphan_check(mode: &str, busy_ms: Option<u64>) -> u32 {
    let dir = tempfile_dir();
    let pid_file = dir.join("worker.pid");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "orphan_stub", "--nocapture", "--test-threads=1"])
        .env(STUB_ENV, &pid_file)
        .env(STUB_MODE_ENV, mode)
        .stdout(std::process::Stdio::null());
    if let Some(ms) = busy_ms {
        command.env(STUB_BUSY_ENV, ms.to_string());
    }
    let mut stub = command.spawn().unwrap();

    let mut waited = Duration::ZERO;
    let pid = loop {
        if let Ok(text) = std::fs::read_to_string(&pid_file)
            && let Ok(pid) = text.trim().parse::<u32>()
        {
            break pid;
        }
        if waited >= Duration::from_secs(20) {
            let _ = stub.kill();
            panic!("the stub never reported a worker");
        }
        std::thread::sleep(Duration::from_millis(20));
        waited += Duration::from_millis(20);
    };
    let worker_alive = alive(pid);
    stub.kill().unwrap();
    stub.wait().unwrap();
    let _ = std::fs::remove_dir_all(dir);
    assert!(
        worker_alive,
        "worker {pid} was not running before its shell was killed"
    );
    pid
}

/// The stand-in shell. A no-op unless run by `orphan_check()`.
pub fn orphan_stub() {
    let Some(pid_file) = std::env::var_os(STUB_ENV) else {
        return;
    };
    let mode = std::env::var(STUB_MODE_ENV).unwrap_or_else(|_| "echo".into());
    let h = Harness::start(fake(&mode));
    h.notice();
    let pid = h.pid();
    let _in_flight = std::env::var(STUB_BUSY_ENV).ok().map(|ms| {
        let pending = h.submit("GET", &format!("boson://app/sleep?ms={ms}"));
        // Long enough for the worker to have read the request and started on it.
        std::thread::sleep(Duration::from_millis(300));
        pending
    });
    std::fs::write(pid_file, pid.to_string()).unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

/// Waits up to `limit` for `pid` to exit; true when it did.
pub fn exits_within(pid: u32, limit: Duration) -> bool {
    let deadline = std::time::Instant::now() + limit;
    while alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    !alive(pid)
}

pub fn tempfile_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "grafida-shell-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
