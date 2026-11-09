//! Starts the shell beside this binary, or explains which system libraries it is missing.
//! See `lib.rs` and grafida/grafida#86.

use std::{
    os::unix::process::CommandExt,
    process::{Command, ExitCode},
};

use grafida_launcher::{Check, alert, check, explain, family, manifest_title, shell_beside};

/// `EX_UNAVAILABLE`, as Grafida's own start-up checks use.
const EXIT_UNAVAILABLE: u8 = 69;

fn main() -> ExitCode {
    let launcher = std::env::current_exe().unwrap_or_else(|_| "grafida-launcher".into());
    let shell = shell_beside(&launcher);
    let title = || {
        manifest_title(&shell.with_file_name("shell.json")).unwrap_or_else(|| {
            launcher
                .file_name()
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
