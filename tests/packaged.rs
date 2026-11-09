//! The packaged layout, with the real binary (grafida/grafida#86): the shell finds its manifest and
//! worker beside its *real* path when it is started through a symlink, and a missing worker is
//! named in the alert rather than surfacing as a spawn error. No display is involved: without one
//! the alert goes to stderr only.

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
    let shell = dir.join("app/grafida-shell");
    if std::fs::hard_link(env!("CARGO_BIN_EXE_grafida-shell"), &shell).is_err() {
        std::fs::copy(env!("CARGO_BIN_EXE_grafida-shell"), &shell).unwrap();
    }
    std::fs::write(dir.join("app/shell.json"), manifest).unwrap();
    dir
}

fn run_headless(program: &Path, cwd: &Path) -> (Option<i32>, String) {
    let out = Command::new(program)
        .current_dir(cwd)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

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
fn no_manifest_beside_the_binary_says_where_it_looked() {
    let dir = install("nomanifest", "{}");
    std::fs::remove_file(dir.join("app/shell.json")).unwrap();
    let (code, stderr) = run_headless(&dir.join("app/grafida-shell"), &dir);
    assert_eq!(code, Some(78), "{stderr}");
    assert!(stderr.contains("No shell.json found"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
