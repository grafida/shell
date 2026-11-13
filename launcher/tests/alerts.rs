//! The alert chain, run for real: the launcher binary with no shell beside it (so the exec fails
//! and it alerts), a fake display, and fake alert tools on `PATH` that record being run.

#![cfg(unix)]

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

const TOOLS: [&str; 4] = ["zenity", "kdialog", "notify-send", "xmessage"];

/// A directory with the launcher (and no shell) and a `bin/` holding the given fake tools, each
/// a script that records its name in `ran` and exits with the given status.
fn setup(name: &str, tools: &[(&str, i32)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("grafida-alerts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_grafida-launcher"),
        dir.join("grafida-launcher"),
    )
    .unwrap();
    for (tool, status) in tools {
        let script = dir.join("bin").join(tool);
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho {tool} >> '{}'\nexit {status}\n",
                dir.join("ran").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

/// Runs the launcher and returns the tools it ran, in order.
fn ran(dir: &Path) -> Vec<String> {
    let status = Command::new(dir.join("grafida-launcher"))
        .env_clear()
        .env("PATH", dir.join("bin"))
        .env("DISPLAY", ":99")
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(69));
    let ran = std::fs::read_to_string(dir.join("ran")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(dir);
    ran.lines().map(str::to_owned).collect()
}

#[test]
fn a_dialog_closed_with_status_1_was_shown() {
    // zenity and kdialog exit 1 when the user closes the dialog: the chain ends there.
    // Only the tools after it in the chain are installed, each one working.
    for tool in ["zenity", "kdialog"] {
        let at = TOOLS.iter().position(|t| *t == tool).unwrap();
        let mut tools = vec![(tool, 1)];
        tools.extend(TOOLS[at + 1..].iter().map(|t| (*t, 0)));
        let dir = setup(tool, &tools);
        assert_eq!(ran(&dir), [tool], "{tool}");
    }
}

#[test]
fn a_notification_that_fails_with_status_1_falls_through_to_xmessage() {
    // notify-send exits 1 on a usage error (and a missing daemon): nothing was shown.
    let dir = setup("notify-send", &[("notify-send", 1), ("xmessage", 0)]);
    assert_eq!(ran(&dir), ["notify-send", "xmessage"]);
}
