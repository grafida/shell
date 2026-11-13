//! The shell's log: its own lines and the worker's stderr, timestamped, in one rotated file.
//!
//! `logDir: "auto"` resolves to the platform's log location for the manifest's `dataDir`
//! (Linux: `$XDG_STATE_HOME/<dataDir>/logs`). Logging is best-effort: a log that cannot be opened or
//! written never stops the app.

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

const FILE_NAME: &str = "shell.log";
const MAX_BYTES: u64 = 5 * 1024 * 1024;
const KEEP: usize = 3;

pub struct Log {
    path: Option<PathBuf>,
    file: Mutex<Option<File>>,
    /// Copy every line to stderr as well (dev mode).
    echo: bool,
}

impl Log {
    /// A log in `dir` (created if needed). With `None`, or when the file cannot be opened, lines go
    /// to stderr only.
    pub fn open(dir: Option<&Path>, echo: bool) -> Log {
        let path = dir.and_then(|d| fs::create_dir_all(d).ok().map(|()| d.join(FILE_NAME)));
        let file = path.as_deref().and_then(open_append);
        if let Some(path) = &path {
            owner_only_rotations(path);
        }
        Log {
            echo: echo || file.is_none(),
            path: if file.is_some() { path } else { None },
            file: Mutex::new(file),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn line(&self, source: &str, text: &str) {
        let line = format!("{} [{source}] {text}\n", timestamp(SystemTime::now()));
        if self.echo {
            eprint!("{line}");
        }
        let Some(path) = &self.path else {
            return;
        };
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .is_some_and(|m| m.len() >= MAX_BYTES)
        {
            *file = None;
            rotate(path);
            *file = open_append(path);
        }
        if let Some(f) = file.as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// The log, opened for appending and readable by its owner only (security.md I3): it holds the
/// worker's stderr, which may carry the user's data. `mode` applies only when the file is created,
/// so a log an older shell created under the umask (usually 0644) is tightened as well. On Windows
/// the per-user `%LOCALAPPDATA%` already keeps other users out.
fn open_append(path: &Path) -> Option<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        let file = options.open(path).ok()?;
        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
        Some(file)
    }
    #[cfg(not(unix))]
    {
        options.open(path).ok()
    }
}

/// `shell.log.1` … `shell.log.3` made owner-only too: an older shell left them at the umask, and they
/// hold its worker's stderr for as long as it takes them to age out.
fn owner_only_rotations(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for n in 1..=KEEP {
            let _ = fs::set_permissions(
                format!("{}.{n}", path.display()),
                fs::Permissions::from_mode(0o600),
            );
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// `shell.log` → `shell.log.1` → … → `shell.log.3`, dropping the oldest.
fn rotate(path: &Path) {
    let numbered = |n: usize| PathBuf::from(format!("{}.{n}", path.display()));
    let _ = fs::remove_file(numbered(KEEP));
    for n in (1..KEEP).rev() {
        let _ = fs::rename(numbered(n), numbered(n + 1));
    }
    let _ = fs::rename(path, numbered(1));
}

/// The platform's log directory for an app's data-dir name.
pub fn auto_dir(data_dir: &str) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let state = std::env::var_os("XDG_STATE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
        Some(state.join(data_dir).join("logs"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Logs").join(data_dir))
    }
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(data_dir).join("Logs"))
    }
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ`, in UTC.
pub fn timestamp(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        let t = UNIX_EPOCH + Duration::from_millis(1_794_220_496_789);
        assert_eq!(timestamp(t), "2026-11-09T10:34:56.789Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400); // 2000-02-29
        assert_eq!(&timestamp(leap)[..10], "2000-02-29");
    }

    #[test]
    fn rotation_keeps_three() {
        let dir = std::env::temp_dir().join(format!("shell-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let log = Log::open(Some(&dir), false);
        let path = log.path().unwrap().to_path_buf();
        for round in 0..5 {
            fs::write(&path, vec![b'x'; MAX_BYTES as usize]).unwrap();
            *log.file.lock().unwrap() = open_append(&path);
            log.line("test", &format!("round {round}"));
        }
        assert!(fs::read_to_string(&path).unwrap().contains("round 4"));
        assert!(Path::new(&format!("{}.3", path.display())).exists());
        assert!(!Path::new(&format!("{}.4", path.display())).exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The log holds the worker's stderr, which may carry the user's data: owner-only, whatever
    /// the umask, on a new file, after a rotation, and on one an older shell left world-readable.
    #[cfg(unix)]
    #[test]
    fn the_log_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;

        let dir = std::env::temp_dir().join(format!("shell-log-mode-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);

        let log = Log::open(Some(&dir), false);
        log.line("test", "new");
        assert_eq!(mode(&path), 0o600, "a new log");

        fs::write(&path, vec![b'x'; MAX_BYTES as usize]).unwrap();
        log.line("test", "rotated");
        assert_eq!(mode(&path), 0o600, "the file after a rotation");
        drop(log);

        fs::remove_file(&path).unwrap();
        let rotated = |n: usize| PathBuf::from(format!("{}.{n}", path.display()));
        for p in [path.clone(), rotated(1), rotated(KEEP)] {
            fs::write(&p, "an older shell's log\n").unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        }
        let log = Log::open(Some(&dir), false);
        log.line("test", "reopened");
        assert_eq!(mode(&path), 0o600, "a log an older shell left at 0644");
        assert_eq!(mode(&rotated(1)), 0o600, "its rotation left at 0644");
        assert_eq!(mode(&rotated(KEEP)), 0o600, "its oldest rotation left at 0644");

        fs::remove_dir_all(&dir).unwrap();
    }
}
