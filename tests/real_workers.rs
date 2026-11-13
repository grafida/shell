//! The supervisor against real PHP workers: grafida/shell-php's echo worker, and Grafida's own
//! `bin/worker.php`. Each test is skipped (with a note on stderr) when its clone or `php` is
//! missing. Point `SHELL_PHP_DIR` / `GRAFIDA_DIR` at the clones; both default to siblings of this
//! repository, as in the operator's checkout.

mod common;

use std::{path::PathBuf, process::Command};

use common::Harness;
use grafida_shell::supervisor::{Notice, WorkerCommand};
use serde_json::Value;

fn clone_dir(var: &str, sibling: &str, marker: &str) -> Option<PathBuf> {
    let dir = std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| common::manifest_dir().join("..").join(sibling));
    let php = Command::new("php")
        .arg("-v")
        .output()
        .is_ok_and(|o| o.status.success());
    if php && dir.join(marker).is_file() && dir.join("vendor/autoload.php").is_file() {
        Some(dir)
    } else {
        eprintln!(
            "skipped: no usable {sibling} clone at {} (or no php); set {var}",
            dir.display()
        );
        None
    }
}

fn php(dir: PathBuf, script: &str, env: Vec<(String, String)>) -> WorkerCommand {
    WorkerCommand {
        program: "php".into(),
        args: vec![script.into()],
        cwd: dir,
        env: env.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
    }
}

#[test]
fn shell_php_echo_worker() {
    let Some(dir) = clone_dir(
        "SHELL_PHP_DIR",
        "shell-php",
        "tests/Fixtures/echo-worker.php",
    ) else {
        return;
    };
    let h = Harness::start(php(dir, "tests/Fixtures/echo-worker.php", vec![]));
    let Notice::Ready { info, first: true } = h.notice() else {
        panic!("expected ready");
    };
    assert_eq!(info.app.as_deref(), Some("echo"));

    for size in [0usize, 1, 1025, 1_048_577, 10 << 20] {
        let body: Vec<u8> = (0..size).map(|i| (i * 7 % 256) as u8).collect();
        let r = h.request(
            "POST",
            "shell://app/api/echo?q=%E2%9C%93",
            &[("x-a", "1"), ("x-a", "2")],
            body.clone(),
        );
        assert_eq!(r.status, 200);
        assert_eq!(r.headers["x-multi"], vec!["one", "two"]);
        let json: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(json["bodyLength"], size);
        assert_eq!(json["bodySha256"], common::sha256(&body));
        assert_eq!(json["headers"]["x-a"], serde_json::json!(["1", "2"]));
    }

    // Stray output stays off the frame stream and reaches the log.
    let r = h.request("GET", "shell://app/stray", &[], vec![]);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"ok");
    let logged: Vec<_> = h.log.try_iter().collect();
    assert!(
        logged
            .iter()
            .any(|(src, line)| src == "worker" && line.contains("[stray-output] stray text")),
        "{logged:?}"
    );

    // A handler that throws is a 500, not a crash.
    assert_eq!(
        h.request("GET", "shell://app/throw", &[], vec![]).status,
        500
    );
    assert_eq!(
        h.request("GET", "shell://app/again", &[], vec![]).status,
        200
    );
}

#[test]
fn grafida_worker() {
    let Some(dir) = clone_dir("GRAFIDA_DIR", "grafida", "bin/worker.php") else {
        return;
    };
    // Never the user's data: every per-user directory points at a throwaway one.
    let home = common::tempfile_dir();
    let env = [
        "HOME",
        "XDG_DATA_HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ]
    .into_iter()
    .map(|k| (k.to_string(), home.to_string_lossy().into_owned()))
    .collect();
    let h = Harness::start(php(dir, "bin/worker.php", env));
    let Notice::Ready { info, .. } = h.notice() else {
        panic!("expected ready");
    };
    assert_eq!(info.app.as_deref(), Some("grafida"));

    let r = h.request("GET", "shell://app/api/bootstrap", &[], vec![]);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let json: Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(json["ok"], true);
    assert!(
        r.headers["cache-control"]
            .iter()
            .any(|v| v.contains("no-store"))
    );

    let r = h.request("GET", "shell://app/", &[], vec![]);
    assert_eq!(r.status, 200);
    assert!(String::from_utf8_lossy(&r.body).contains("<html"));

    h.shutdown();
    let _ = std::fs::remove_dir_all(home);
}
