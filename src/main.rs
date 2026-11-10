//! Grafida Shell: a window, a webview serving `<scheme>://app`, and a supervised worker process.
//! See docs/adr/0001-shell-stack.md, grafida/grafida#82 (design) and #85 (Linux, dev mode).

// A GUI-subsystem program on Windows: no console window of its own (grafida/grafida#91).
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::{path::PathBuf, process::ExitCode, sync::Arc};

use grafida_shell::{
    cli::{self, Command},
    log::{self, Log},
    protocol::PROTOCOL_VERSION,
};

/// `EX_CONFIG`: the manifest or the command line is wrong.
const EXIT_CONFIG: u8 = 78;

fn main() -> ExitCode {
    let options = match cli::parse(std::env::args_os()) {
        Ok(Command::Run(options)) => options,
        Ok(Command::Version) => {
            println!(
                "{} {} (protocol {PROTOCOL_VERSION})",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION")
            );
            return ExitCode::SUCCESS;
        }
        Ok(Command::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("{e}\n\n{}", cli::USAGE);
            return ExitCode::from(EXIT_CONFIG);
        }
    };

    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("grafida-shell"));
    let launch = match options.resolve(&exe) {
        Ok(launch) => launch,
        Err(e) => {
            platform::alert(
                &e.title().map_or_else(|| shell_name(&exe), str::to_owned),
                "The application cannot start.",
                &e.to_string(),
            );
            return ExitCode::from(EXIT_CONFIG);
        }
    };

    if launch.automation && !cfg!(all(feature = "automation", debug_assertions)) {
        platform::alert(
            &launch.manifest.title,
            "--automation is not available.",
            "This build of the shell has no automation channel.",
        );
        return ExitCode::from(EXIT_CONFIG);
    }

    // SAFETY: no thread has been started yet (the log, the supervisor and GTK all come later).
    let notes = unsafe { platform::prepare(&launch.manifest) };

    let log_dir = match launch.manifest.log_dir.as_str() {
        "auto" => log::auto_dir(launch.manifest.data_dir()),
        dir => {
            let dir = PathBuf::from(dir);
            Some(if dir.is_absolute() {
                dir
            } else {
                launch.worker.cwd.join(dir)
            })
        }
    };
    let log = Arc::new(Log::open(log_dir.as_deref(), launch.dev));
    log.line(
        "shell",
        &format!(
            "starting {} from {}{}",
            launch.manifest.id,
            launch.manifest_path.display(),
            if launch.dev { " (dev mode)" } else { "" }
        ),
    );
    for note in notes {
        log.line("shell", &note);
    }

    let code = platform::run(launch, log);
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn shell_name(exe: &std::path::Path) -> String {
    exe.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Shell".into())
}

#[cfg(target_os = "linux")]
mod platform {
    pub use grafida_shell::linux::{alert, environment::prepare, run};
}

#[cfg(target_os = "macos")]
mod platform {
    pub use grafida_shell::macos::{alert, prepare, run};
}

#[cfg(windows)]
mod platform {
    pub use grafida_shell::windows::{alert, prepare, run};
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use grafida_shell::{cli::Launch, log::Log, manifest::Manifest};
    use std::sync::Arc;

    /// # Safety
    /// Nothing to set up yet; kept `unsafe` to match the Linux signature.
    pub unsafe fn prepare(_manifest: &Manifest) -> Vec<String> {
        Vec::new()
    }

    pub fn alert(title: &str, message: &str, detail: &str) {
        eprintln!("{title}: {message}\n{detail}");
    }

    pub fn run(_launch: Launch, _log: Arc<Log>) -> i32 {
        alert(
            "Shell",
            "This platform is not supported yet.",
            "grafida/grafida#82: Linux, macOS and Windows only.",
        );
        1
    }
}
