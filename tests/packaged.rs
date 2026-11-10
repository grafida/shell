//! The packaged layout, with the real binary (grafida/grafida#86): the shell finds its manifest and
//! worker beside its *real* path when it is started through a symlink, and a missing worker is
//! named in the alert rather than surfacing as a spawn error. No alert is shown: with
//! `SHELL_ALERTS=stderr` (and on Linux without a display) it goes to stderr only.

use std::{path::Path, process::Command};

fn manifest(worker: &str) -> String {
    format!(
        r#"{{"id":"test.shell.packaged","title":"Packaged test","scheme":"boson",
        "window":{{"width":10,"height":10}},"worker":{{"binary":"{worker}"}}}}"#
    )
}

/// A copy of the built shell in a fresh "install" directory, beside a manifest.
fn install(name: &str, manifest: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shell-packaged-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("app")).unwrap();
    let shell = dir.join(format!("app/grafida-shell{}", std::env::consts::EXE_SUFFIX));
    if std::fs::hard_link(env!("CARGO_BIN_EXE_grafida-shell"), &shell).is_err() {
        std::fs::copy(env!("CARGO_BIN_EXE_grafida-shell"), &shell).unwrap();
    }
    std::fs::write(dir.join("app/shell.json"), manifest).unwrap();
    dir
}

fn run_headless(program: &Path, cwd: &Path) -> (Option<i32>, String) {
    let run = || {
        Command::new(program)
            .current_dir(cwd)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env("SHELL_ALERTS", "stderr")
            .output()
    };
    // The tests copy the shell in parallel: another test's fork can briefly hold the copy open for
    // writing, and exec then fails with ETXTBSY. That is the test's race, not the shell's.
    let mut out = run();
    for _ in 0..20 {
        match &out {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(50));
                out = run();
            }
            _ => break,
        }
    }
    let out = out.unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[cfg(unix)]
#[test]
fn a_symlinked_shell_finds_its_manifest_and_names_the_missing_worker() {
    let dir = install("symlink", &manifest("t-worker"));
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let link = dir.join("bin/t");
    std::os::unix::fs::symlink(dir.join("app/grafida-shell"), &link).unwrap();

    // Started from elsewhere, through the link: the manifest beside the real file is found (the
    // title is in the alert), and the worker is expected beside it too.
    let (code, stderr) = run_headless(&link, Path::new("/"));
    assert_eq!(code, Some(78), "{stderr}");
    assert!(stderr.contains("Packaged test"), "{stderr}");
    assert!(stderr.contains("`t-worker` is missing"), "{stderr}");
    let real = std::fs::canonicalize(dir.join("app")).unwrap();
    assert!(
        stderr.contains(&real.join("t-worker").display().to_string()),
        "{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_worker_is_named_with_the_path_it_was_expected_at() {
    // Everywhere, and on Windows (no symlink without privileges) the packaged check itself: the
    // worker `t-worker` is looked for beside the shell, as `t-worker.exe` on Windows.
    let dir = install("missing", &manifest("t-worker"));
    let shell = dir.join(format!("app/grafida-shell{}", std::env::consts::EXE_SUFFIX));
    let (code, stderr) = run_headless(&shell, &dir);
    assert_eq!(code, Some(78), "{stderr}");
    assert!(stderr.contains("`t-worker` is missing"), "{stderr}");
    let expected = format!("t-worker{}", std::env::consts::EXE_SUFFIX);
    assert!(stderr.contains(&expected), "{stderr}");
    assert!(
        !stderr.contains(r"\\?\"),
        "a verbatim path in the alert: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_manifest_beside_the_binary_says_where_it_looked() {
    let dir = install("nomanifest", "{}");
    std::fs::remove_file(dir.join("app/shell.json")).unwrap();
    let shell = dir.join(format!("app/grafida-shell{}", std::env::consts::EXE_SUFFIX));
    let (code, stderr) = run_headless(&shell, &dir);
    assert_eq!(code, Some(78), "{stderr}");
    assert!(stderr.contains("No shell.json found"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
