//! Starts the shell beside this binary, or explains which system libraries it is missing.
//! See `lib.rs` and grafida/grafida#86.

#[cfg(unix)]
use std::{
    os::unix::process::CommandExt,
    process::{Command, ExitCode},
};

#[cfg(unix)]
use grafida_launcher::{Check, alert, check, explain, family, manifest_title, shell_beside};

/// `EX_UNAVAILABLE`, as Grafida's own start-up checks use.
const EXIT_UNAVAILABLE: u8 = 69;

/// The launcher exists for Linux, where the shell may not load at all (a missing WebKitGTK). On
/// Windows the shell links only system DLLs and checks for WebView2 itself (grafida/grafida#91), so
/// nothing ships this binary there; it builds only so the workspace does.
#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    eprintln!("grafida-launcher is for Linux: on this platform the shell is started directly.");
    std::process::ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(unix)]
fn main() -> ExitCode {
    // Without an absolute path to ourselves there is no "beside": refuse rather than let the shell
    // be searched for on PATH (`/proc` missing, e.g. a chroot).
    let launcher = std::env::current_exe().ok();
    let Some(shell) = launcher.as_deref().and_then(shell_beside) else {
        let title = "The application";
        alert(
            title,
            &format!("{title} cannot start."),
            "The launcher could not find out where it is installed, so it cannot locate the \
             application beside it.",
        );
        return ExitCode::from(EXIT_UNAVAILABLE);
    };
    let title = || {
        manifest_title(&shell.with_file_name("shell.json")).unwrap_or_else(|| {
            launcher
                .as_deref()
                .and_then(|l| l.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "The application".into())
        })
    };

    match check(&shell) {
        Check::Loads => {}
        Check::Refused(problems) => {
            let os_release = std::fs::read_to_string("/etc/os-release")
                .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
                .unwrap_or_default();
            let title = title();
            let (message, detail) = explain(&title, &problems, family(&os_release));
            alert(&title, &message, &detail);
            return ExitCode::from(EXIT_UNAVAILABLE);
        }
        // Fail open: the shell's own failure, if any, is still better than refusing on a guess.
        Check::Unknown(why) => eprintln!("launcher: could not check the shell's libraries: {why}"),
    }

    let error = Command::new(&shell)
        .args(std::env::args_os().skip(1))
        .exec();
    let title = title();
    alert(
        &title,
        &format!("{title} cannot start."),
        &format!("{}: {error}", shell.display()),
    );
    ExitCode::from(EXIT_UNAVAILABLE)
}
