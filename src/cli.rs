//! Command line and launch resolution: which manifest, which worker, which working directory.
//!
//! ```text
//! grafida-shell                      packaged: shell.json beside the binary (or ../Resources),
//!                                    the worker beside the binary
//! grafida-shell --dev [APP_DIR]      dev: APP_DIR/shell.json, worker = its dev.worker, cwd = APP_DIR
//!               --manifest PATH      use this manifest instead
//!               --automation         debug builds with the `automation` feature only
//! ```

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    manifest::{self, Manifest, ManifestError},
    supervisor::WorkerCommand,
};

pub const USAGE: &str = "\
Usage: grafida-shell [--dev [APP_DIR]] [--manifest PATH] [--automation]

  --dev [APP_DIR]   Run an app from its source tree: read APP_DIR/shell.json (default: the
                    current directory) and start the manifest's dev.worker command there.
  --manifest PATH   Read this manifest instead.
  --automation      Open the loopback automation channel (debug builds only).
  --version         Print the shell and protocol versions.
  --help            Print this text.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run(Options),
    Version,
    Help,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// `Some(dir)` in dev mode.
    pub dev: Option<PathBuf>,
    pub manifest: Option<PathBuf>,
    pub automation: bool,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
    let mut args = args.into_iter().skip(1).peekable();
    let mut options = Options::default();

    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => return Ok(Command::Help),
            Some("--version" | "-V") => return Ok(Command::Version),
            Some("--dev") => {
                let dir = match args.peek() {
                    Some(next) if !next.to_string_lossy().starts_with("--") => {
                        PathBuf::from(args.next().unwrap())
                    }
                    _ => PathBuf::from("."),
                };
                options.dev = Some(dir);
            }
            Some("--manifest") => {
                let path = args.next().ok_or("--manifest needs a path")?;
                options.manifest = Some(PathBuf::from(path));
            }
            Some("--automation") => options.automation = true,
            _ => return Err(format!("unknown argument: {}", arg.to_string_lossy())),
        }
    }
    Ok(Command::Run(options))
}

/// Everything the shell needs to start, resolved from the options.
#[derive(Debug, Clone)]
pub struct Launch {
    pub manifest_path: PathBuf,
    pub manifest: Manifest,
    pub worker: WorkerCommand,
    pub dev: bool,
    pub automation: bool,
}

#[derive(Debug)]
pub enum LaunchError {
    /// The manifest could not be found, read or validated; carries its path.
    Manifest(PathBuf, ManifestError),
    NoManifest(Vec<PathBuf>),
    /// The manifest loaded, but its worker is not beside the shell: (the app's title, the
    /// worker's name, the path it was looked for at).
    MissingWorker(String, String, PathBuf),
    Other(String),
}

impl LaunchError {
    /// The app's title, when the manifest got as far as loading.
    pub fn title(&self) -> Option<&str> {
        match self {
            LaunchError::MissingWorker(title, ..) => Some(title),
            _ => None,
        }
    }
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::Manifest(path, e) => write!(f, "The manifest {} {e}", path.display()),
            LaunchError::NoManifest(tried) => {
                write!(f, "No {} found. Looked in:", manifest::FILE_NAME)?;
                for p in tried {
                    write!(f, "\n  • {}", p.display())?;
                }
                Ok(())
            }
            LaunchError::MissingWorker(_, binary, path) => write!(
                f,
                "The worker `{binary}` is missing. It should be beside the shell, at {}.",
                path.display()
            ),
            LaunchError::Other(e) => f.write_str(e),
        }
    }
}

impl Options {
    /// Finds and loads the manifest and works out the worker command. `exe` is the shell's own path;
    /// everything is resolved from its real location, so a symlink to the shell works.
    pub fn resolve(&self, exe: &Path) -> Result<Launch, LaunchError> {
        let exe = real_path(exe);
        let exe = exe.as_path();
        let manifest_path = match (&self.manifest, &self.dev) {
            (Some(path), _) => absolute(path),
            (None, Some(dir)) => absolute(&dir.join(manifest::FILE_NAME)),
            (None, None) => {
                let candidates = packaged_candidates(exe);
                let found = candidates
                    .iter()
                    .find(|p| p.is_file())
                    .ok_or_else(|| LaunchError::NoManifest(candidates.clone()))?;
                // No `..` left in it (the macOS candidate has one): every path derived from the
                // manifest's directory, the worker's included, should be the real one.
                real_path(found)
            }
        };

        let manifest = Manifest::load(&manifest_path)
            .map_err(|e| LaunchError::Manifest(manifest_path.clone(), e))?;

        let worker = match &self.dev {
            Some(dir) => {
                let dir = absolute(dir);
                let Some([program, args @ ..]) = manifest.dev.worker.as_deref() else {
                    return Err(LaunchError::Other(format!(
                        "--dev needs `dev.worker` in {}",
                        manifest_path.display()
                    )));
                };
                WorkerCommand {
                    program: program.into(),
                    args: args.iter().map(OsString::from).collect(),
                    cwd: dir,
                    env: Vec::new(),
                }
            }
            None => {
                // The worker is a program, so it sits beside the shell: the same directory as the
                // manifest on Linux and Windows, `Contents/MacOS` in a macOS bundle, where
                // codesign wants code and nothing else (grafida/grafida#88). It still runs in the
                // manifest's directory, where the app's data files are.
                let resources = manifest_path
                    .parent()
                    .unwrap_or(Path::new("/"))
                    .to_path_buf();
                let program = with_exe_suffix(
                    exe.parent()
                        .unwrap_or(Path::new("/"))
                        .join(&manifest.worker.binary),
                );
                if !program.is_file() {
                    return Err(LaunchError::MissingWorker(
                        manifest.title.clone(),
                        manifest.worker.binary.clone(),
                        program,
                    ));
                }
                WorkerCommand {
                    program: program.into(),
                    args: manifest.worker.args.iter().map(OsString::from).collect(),
                    cwd: resources,
                    env: Vec::new(),
                }
            }
        };

        Ok(Launch {
            manifest_path,
            manifest,
            worker,
            dev: self.dev.is_some(),
            automation: self.automation,
        })
    }
}

/// Packaged layouts: beside the binary (Linux, Windows), or `../Resources` (a macOS bundle).
fn packaged_candidates(exe: &Path) -> Vec<PathBuf> {
    let dir = exe.parent().unwrap_or(Path::new("."));
    vec![
        dir.join(manifest::FILE_NAME),
        dir.join("../Resources").join(manifest::FILE_NAME),
    ]
}

/// A path with every symlink resolved, as the system names it. On Windows that is without the
/// verbatim `\\?\` prefix `canonicalize` adds (grafida/grafida#91): the worker is started by that
/// path and in that directory, and phpmicro derives its sibling `.phar` from its own path.
fn real_path(path: &Path) -> PathBuf {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match real.to_str().and_then(strip_verbatim) {
        Some(plain) => PathBuf::from(plain),
        None => real,
    }
}

/// `\\?\C:\x` → `C:\x` and `\\?\UNC\server\share\x` → `\\server\share\x`; `None` for anything
/// else, including a verbatim path that cannot be written plainly (a component Win32 would
/// reinterpret, or one too long without the prefix).
fn strip_verbatim(path: &str) -> Option<String> {
    let plain = if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        let rest = path.strip_prefix(r"\\?\")?;
        let drive = rest.as_bytes();
        if drive.len() < 3 || !drive[0].is_ascii_alphabetic() || &drive[1..3] != b":\\" {
            return None;
        }
        rest.to_string()
    };
    const MAX_PATH: usize = 260;
    let reinterpreted = plain.split('\\').skip(1).any(|c| {
        c == "." || c == ".." || c.ends_with(['.', ' ']) || {
            let stem = c.split('.').next().unwrap_or("").to_ascii_uppercase();
            matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.as_bytes()[3].is_ascii_digit())
        }
    });
    (!reinterpreted && plain.len() < MAX_PATH).then_some(plain)
}

/// On Windows a program is `<name>.exe`: a manifest names the worker the same way on every
/// platform (`grafida-worker`), so a name without an extension gets `.exe` there.
fn with_exe_suffix(program: PathBuf) -> PathBuf {
    if cfg!(windows) && program.extension().is_none() {
        program.with_extension("exe")
    } else {
        program
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<Command, String> {
        parse(
            std::iter::once("shell")
                .chain(list.iter().copied())
                .map(OsString::from),
        )
    }

    #[test]
    fn verbatim_paths_are_written_plainly_when_they_can_be() {
        assert_eq!(
            strip_verbatim(r"\\?\C:\Program Files\App\shell.json").as_deref(),
            Some(r"C:\Program Files\App\shell.json")
        );
        assert_eq!(
            strip_verbatim(r"\\?\UNC\srv\share\app").as_deref(),
            Some(r"\\srv\share\app")
        );
        assert_eq!(strip_verbatim(r"C:\already\plain"), None);
        assert_eq!(strip_verbatim(r"\\?\Volume{0a1b}\x"), None);
        assert_eq!(strip_verbatim(r"\\?\C:\dir\NUL.txt"), None, "a device name");
        assert_eq!(
            strip_verbatim(r"\\?\C:\dir\trailing."),
            None,
            "a trailing dot"
        );
        assert_eq!(strip_verbatim(r"\\?\C:\dir\com1"), None);
        assert!(
            strip_verbatim(r"\\?\C:\dir\COMMON\x").is_some(),
            "not a device"
        );
        let long = format!(r"\\?\C:\{}", "a".repeat(300));
        assert_eq!(strip_verbatim(&long), None, "too long without the prefix");
    }

    #[test]
    fn a_worker_name_without_an_extension_is_an_exe_on_windows() {
        let named = with_exe_suffix(PathBuf::from("app/grafida-worker"));
        if cfg!(windows) {
            assert_eq!(named, PathBuf::from("app/grafida-worker.exe"));
        } else {
            assert_eq!(named, PathBuf::from("app/grafida-worker"));
        }
        assert_eq!(
            with_exe_suffix(PathBuf::from("app/w.cmd")),
            PathBuf::from("app/w.cmd")
        );
    }

    #[test]
    fn parsing() {
        assert_eq!(args(&[]), Ok(Command::Run(Options::default())));
        assert_eq!(args(&["--version"]), Ok(Command::Version));
        let Ok(Command::Run(o)) = args(&["--dev"]) else {
            panic!()
        };
        assert_eq!(o.dev, Some(PathBuf::from(".")));
        let Ok(Command::Run(o)) = args(&["--dev", "../app", "--automation"]) else {
            panic!()
        };
        assert_eq!(o.dev, Some(PathBuf::from("../app")));
        assert!(o.automation);
        let Ok(Command::Run(o)) = args(&["--dev", "--manifest", "/x/shell.json"]) else {
            panic!()
        };
        assert_eq!(o.dev, Some(PathBuf::from(".")));
        assert_eq!(o.manifest, Some(PathBuf::from("/x/shell.json")));
        assert!(args(&["--frobnicate"]).is_err());
        assert!(args(&["--manifest"]).is_err());
    }

    fn app_dir(manifest: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "shell-cli-test-{}-{}",
            std::process::id(),
            crate::tokens::random_hex(4)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shell.json"), manifest).unwrap();
        // The real path: macOS's temp dir is behind a symlink (/var → /private/var); on Windows
        // written plainly, as the shell itself writes it.
        real_path(&dir)
    }

    /// The test worker's file name: `t-worker`, `t-worker.exe` on Windows.
    fn worker() -> String {
        format!("t-worker{}", std::env::consts::EXE_SUFFIX)
    }

    const MANIFEST: &str = r#"{"id":"app.t","title":"T","scheme":"boson",
        "window":{"width":10,"height":10},"worker":{"binary":"t-worker","args":["-q"]},
        "dev":{"worker":["php","bin/worker.php"]}}"#;

    #[test]
    fn dev_mode_runs_the_dev_worker_in_the_app_dir() {
        let dir = app_dir(MANIFEST);
        let o = Options {
            dev: Some(dir.clone()),
            ..Options::default()
        };
        let launch = o.resolve(Path::new("/opt/shell/grafida-shell")).unwrap();
        assert_eq!(launch.worker.program, OsString::from("php"));
        assert_eq!(launch.worker.args, vec![OsString::from("bin/worker.php")]);
        assert_eq!(launch.worker.cwd, dir);
        assert!(launch.dev);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn packaged_mode_finds_the_manifest_beside_the_binary() {
        let dir = app_dir(MANIFEST);
        std::fs::write(dir.join(worker()), "").unwrap();
        let launch = Options::default()
            .resolve(&dir.join("grafida-shell"))
            .unwrap();
        assert_eq!(launch.worker.program, dir.join(worker()).into_os_string());
        assert_eq!(launch.worker.args, vec![OsString::from("-q")]);
        assert_eq!(launch.worker.cwd, dir);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn packaged_mode_finds_a_macos_bundles_resources_without_dot_dot() {
        let bundle = app_dir("{}");
        let contents = bundle.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::create_dir_all(contents.join("Resources")).unwrap();
        std::fs::write(contents.join("Resources/shell.json"), MANIFEST).unwrap();
        // A worker among the data files is not the worker: it belongs beside the shell.
        std::fs::write(contents.join("Resources").join(worker()), "").unwrap();
        let shell = contents.join("MacOS/grafida-shell");
        let err = Options::default().resolve(&shell).unwrap_err().to_string();
        assert!(
            err.contains(&contents.join("MacOS").join(worker()).display().to_string()),
            "{err}"
        );

        std::fs::write(contents.join("MacOS").join(worker()), "").unwrap();
        let launch = Options::default().resolve(&shell).unwrap();
        let resources = contents.join("Resources");
        assert_eq!(launch.manifest_path, resources.join("shell.json"));
        assert_eq!(launch.worker.cwd, resources);
        assert_eq!(
            launch.worker.program,
            contents.join("MacOS").join(worker()).into_os_string()
        );
        std::fs::remove_dir_all(bundle).unwrap();
    }

    #[test]
    fn packaged_mode_names_a_missing_worker() {
        let dir = app_dir(MANIFEST);
        let err = Options::default()
            .resolve(&dir.join("grafida-shell"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("`t-worker` is missing"), "{err}");
        assert!(
            err.contains(&dir.join(worker()).display().to_string()),
            "{err}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dev_mode_without_a_dev_worker_is_an_error() {
        let dir = app_dir(
            r#"{"id":"app.t","title":"T","scheme":"boson","window":{"width":10,"height":10},"worker":{"binary":"w"}}"#,
        );
        let o = Options {
            dev: Some(dir.clone()),
            ..Options::default()
        };
        let err = o.resolve(Path::new("/x")).unwrap_err().to_string();
        assert!(err.contains("dev.worker"), "{err}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_missing_manifest_lists_where_it_looked() {
        let err = Options::default()
            .resolve(Path::new("/nonexistent/bin/shell"))
            .unwrap_err()
            .to_string();
        let expected = Path::new("/nonexistent/bin").join("shell.json");
        assert!(err.contains(&expected.display().to_string()), "{err}");
        assert!(err.contains("Resources"), "{err}");
    }
}
