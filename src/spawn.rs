//! Starting the worker process with its three pipes, and tying it to the shell.
//!
//! Unix uses std's `Command`. Windows calls `CreateProcessW` itself (grafida/grafida#91), for two
//! things std cannot do on stable Rust:
//!
//! - **`STARTF_FORCEOFFFEEDBACK`.** Windows shows the "working in background" pointer for a GUI
//!   program it has just started until that program shows a window, and the packaged worker is a
//!   GUI-subsystem program that never does: every launch showed the busy pointer for about 5 s
//!   (measured on Windows 10 22H2; with the same worker flipped to the console subsystem, 0.1 s).
//! - **The main thread's handle.** The worker is created suspended, put in the shell's Job Object,
//!   then resumed, so no instruction of it runs outside the job; `CreateProcessW` hands back the
//!   thread, which std keeps to itself (it used to be found with a Toolhelp snapshot).
//!
//! Only the three pipe ends are inherited: they are named in `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`,
//! so an inheritable handle created elsewhere in the shell can never leak into the worker (and a
//! leaked write end of its stdout would hide its death from the supervisor).

use std::{
    ffi::{OsStr, OsString},
    io::{self, Read},
    path::Path,
};

/// The worker, started: its process and its three pipes (the shell's ends).
pub struct Spawned {
    pub child: WorkerChild,
    pub stdin: WorkerStdin,
    pub stdout: Box<dyn Read + Send>,
    pub stderr: Box<dyn Read + Send>,
}

/// Why the worker did not start.
pub enum SpawnError {
    /// The process could not be created.
    Start(io::Error),
    /// It was created, but could not be tied to the shell (and was killed).
    Tie(io::Error),
}

#[cfg(not(windows))]
pub type WorkerChild = std::process::Child;
#[cfg(not(windows))]
pub type WorkerStdin = std::process::ChildStdin;
#[cfg(windows)]
pub use self::windows::WorkerChild;
#[cfg(windows)]
pub type WorkerStdin = std::fs::File;

/// Starts `program args…` in `cwd`, with the shell's environment plus `env`.
pub fn spawn(
    program: &OsStr,
    args: &[OsString],
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> Result<Spawned, SpawnError> {
    imp(program, args, cwd, env)
}

#[cfg(not(windows))]
fn imp(
    program: &OsStr,
    args: &[OsString],
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> Result<Spawned, SpawnError> {
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    tie_to_parent(&mut cmd);
    let mut child = cmd.spawn().map_err(SpawnError::Start)?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    Ok(Spawned {
        child,
        stdin,
        stdout: Box::new(stdout),
        stderr: Box::new(stderr),
    })
}

/// The worker must never outlive the shell.
#[cfg(target_os = "linux")]
fn tie_to_parent(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: getpid is async-signal-safe and called before the fork.
    let parent = unsafe { libc::getpid() };
    // SAFETY: the closure runs between fork and exec and calls only async-signal-safe functions
    // (prctl, getppid, raise).
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(io::Error::last_os_error());
            }
            // The shell died between fork and prctl: the signal will never come, so act now.
            if libc::getppid() != parent {
                libc::raise(libc::SIGTERM);
            }
            Ok(())
        });
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn tie_to_parent(_cmd: &mut std::process::Command) {
    // macOS: EOF on stdin only (grafida/grafida#88): an idle worker exits when the shell dies, a
    // busy one when its request ends.
}

#[cfg(windows)]
fn imp(
    program: &OsStr,
    args: &[OsString],
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> Result<Spawned, SpawnError> {
    self::windows::spawn(program, args, cwd, env)
}

/// Appends `arg` to a Windows command line, quoted the way the MSVC runtime (and std) reads it
/// back: quotes around an empty argument or one with a space or tab, every `"` escaped, and the
/// backslashes before a quote (or before the closing quote) doubled.
#[cfg(any(windows, test))]
fn append_arg(line: &mut Vec<u16>, arg: &[u16], force_quotes: bool) {
    const QUOTE: u16 = b'"' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    let quote = force_quotes
        || arg.is_empty()
        || arg.iter().any(|&c| c == b' ' as u16 || c == b'\t' as u16);
    if quote {
        line.push(QUOTE);
    }
    let mut backslashes = 0;
    for &c in arg {
        if c == BACKSLASH {
            backslashes += 1;
        } else {
            if c == QUOTE {
                line.extend(std::iter::repeat_n(BACKSLASH, backslashes + 1));
            }
            backslashes = 0;
        }
        line.push(c);
    }
    if quote {
        line.extend(std::iter::repeat_n(BACKSLASH, backslashes));
        line.push(QUOTE);
    }
}

#[cfg(windows)]
mod windows {
    use std::{
        collections::BTreeMap,
        ffi::{OsStr, OsString, c_void},
        fs::File,
        io,
        os::windows::{
            ffi::{OsStrExt, OsStringExt},
            io::{FromRawHandle, RawHandle},
            process::ExitStatusExt,
        },
        path::{Path, PathBuf},
        process::ExitStatus,
        sync::OnceLock,
    };

    use windows::{
        Win32::{
            Foundation::{
                CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0,
            },
            Security::SECURITY_ATTRIBUTES,
            System::{
                JobObjects::{
                    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                    SetInformationJobObject,
                },
                Pipes::CreatePipe,
                Threading::{
                    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
                    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
                    GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
                    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                    PROCESS_INFORMATION, ResumeThread, STARTF_FORCEOFFFEEDBACK,
                    STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
                    UpdateProcThreadAttribute, WaitForSingleObject,
                },
            },
        },
        core::{PCWSTR, PWSTR},
    };

    use super::{SpawnError, Spawned, append_arg};

    /// The worker's process. Closing it does not end the worker; the job does, with the shell.
    pub struct WorkerChild {
        process: HANDLE,
        pid: u32,
    }

    // SAFETY: a process handle is a kernel object handle, usable from any thread.
    unsafe impl Send for WorkerChild {}

    impl WorkerChild {
        pub fn id(&self) -> u32 {
            self.pid
        }

        pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            // SAFETY: the handle is live for as long as `self` is.
            if unsafe { WaitForSingleObject(self.process, 0) } == WAIT_OBJECT_0 {
                self.status().map(Some)
            } else {
                Ok(None)
            }
        }

        pub fn wait(&mut self) -> io::Result<ExitStatus> {
            // SAFETY: as above.
            if unsafe { WaitForSingleObject(self.process, INFINITE) } != WAIT_OBJECT_0 {
                return Err(io::Error::last_os_error());
            }
            self.status()
        }

        /// `TerminateProcess` with exit code 1, as std's `Child::kill`; a worker already gone is
        /// not an error.
        pub fn kill(&mut self) -> io::Result<()> {
            // SAFETY: as above.
            match unsafe { TerminateProcess(self.process, 1) } {
                Ok(()) => Ok(()),
                Err(_) if self.try_wait()?.is_some() => Ok(()),
                Err(e) => Err(io::Error::other(e.message())),
            }
        }

        fn status(&self) -> io::Result<ExitStatus> {
            let mut code = 0u32;
            // SAFETY: as above; `code` outlives the call.
            unsafe { GetExitCodeProcess(self.process, &mut code) }
                .map_err(|e| io::Error::other(e.message()))?;
            Ok(ExitStatus::from_raw(code))
        }
    }

    impl Drop for WorkerChild {
        fn drop(&mut self) {
            // SAFETY: ours to close, once.
            let _ = unsafe { CloseHandle(self.process) };
        }
    }

    /// Closes a handle when dropped, unless taken.
    struct Owned(HANDLE);

    impl Owned {
        fn take(mut self) -> HANDLE {
            std::mem::take(&mut self.0)
        }
    }

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_invalid() && !self.0.0.is_null() {
                // SAFETY: ours to close, once.
                let _ = unsafe { CloseHandle(self.0) };
            }
        }
    }

    /// An anonymous pipe: (the shell's end, the worker's end). Neither is inheritable yet.
    fn pipe(worker_reads: bool) -> io::Result<(Owned, Owned)> {
        let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: false.into(),
        };
        // SAFETY: both out-pointers and the attributes outlive the call.
        unsafe { CreatePipe(&mut read, &mut write, Some(&attributes), 0) }
            .map_err(|e| io::Error::other(format!("CreatePipe: {}", e.message())))?;
        let (read, write) = (Owned(read), Owned(write));
        Ok(if worker_reads {
            (write, read)
        } else {
            (read, write)
        })
    }

    pub fn spawn(
        program: &OsStr,
        args: &[OsString],
        cwd: &Path,
        env: &[(OsString, OsString)],
    ) -> Result<Spawned, SpawnError> {
        let start = |e: io::Error| SpawnError::Start(e);

        let block = environment(env);
        let exe = resolve(program, cwd, &block).map_err(start)?;
        let mut line = Vec::new();
        append_arg(
            &mut line,
            &exe.as_os_str().encode_wide().collect::<Vec<_>>(),
            true,
        );
        for arg in args {
            line.push(b' ' as u16);
            append_arg(&mut line, &arg.encode_wide().collect::<Vec<_>>(), false);
        }
        line.push(0);
        let exe_w: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
        let cwd_w: Vec<u16> = cwd.as_os_str().encode_wide().chain([0]).collect();

        let (stdin_ours, stdin_theirs) = pipe(true).map_err(start)?;
        let (stdout_ours, stdout_theirs) = pipe(false).map_err(start)?;
        let (stderr_ours, stderr_theirs) = pipe(false).map_err(start)?;
        let theirs = [stdin_theirs.0, stdout_theirs.0, stderr_theirs.0];
        for h in theirs {
            // SAFETY: a live pipe handle of ours; it becomes inheritable only for this spawn, and
            // the handle list below keeps it out of any other child.
            unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) }
                .map_err(|e| {
                    start(io::Error::other(format!(
                        "SetHandleInformation: {}",
                        e.message()
                    )))
                })?;
        }

        // The attribute list: one attribute, the handles the worker may inherit.
        let mut size = 0usize;
        // SAFETY: the sizing call, which reports the size it needs through `size`.
        let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut size) };
        let mut buffer = vec![0u8; size];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(buffer.as_mut_ptr().cast());
        // SAFETY: `buffer` has the size the sizing call asked for and outlives the list.
        unsafe { InitializeProcThreadAttributeList(Some(list), 1, None, &mut size) }.map_err(
            |e| {
                start(io::Error::other(format!(
                    "InitializeProcThreadAttributeList: {}",
                    e.message()
                )))
            },
        )?;
        // SAFETY: `theirs` outlives the CreateProcessW call that reads it through the list.
        let updated = unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(theirs.as_ptr().cast::<c_void>()),
                size_of_val(&theirs),
                None,
                None,
            )
        };

        let mut info = STARTUPINFOEXW::default();
        info.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        info.StartupInfo.dwFlags = STARTF_USESTDHANDLES | STARTF_FORCEOFFFEEDBACK;
        info.StartupInfo.hStdInput = theirs[0];
        info.StartupInfo.hStdOutput = theirs[1];
        info.StartupInfo.hStdError = theirs[2];
        info.lpAttributeList = list;
        let mut process = PROCESS_INFORMATION::default();

        let created = updated.and_then(|()| {
            // SAFETY: every pointer (the command line, the environment block, the directory, the
            // startup information and its attribute list) outlives the call.
            unsafe {
                CreateProcessW(
                    PCWSTR(exe_w.as_ptr()),
                    Some(PWSTR(line.as_mut_ptr())),
                    None,
                    None,
                    true,
                    CREATE_SUSPENDED
                        | CREATE_NO_WINDOW
                        | CREATE_UNICODE_ENVIRONMENT
                        | EXTENDED_STARTUPINFO_PRESENT,
                    Some(block.as_ptr().cast()),
                    PCWSTR(cwd_w.as_ptr()),
                    &info.StartupInfo,
                    &mut process,
                )
            }
        });
        // SAFETY: initialised above, deleted once, after its last use.
        unsafe { DeleteProcThreadAttributeList(list) };
        // The worker has its copies; ours go now, or its stdout would never reach EOF.
        drop((stdin_theirs, stdout_theirs, stderr_theirs));
        created.map_err(|e| start(io::Error::from_raw_os_error(e.code().0 & 0xFFFF)))?;

        let thread = Owned(process.hThread);
        let mut child = WorkerChild {
            process: process.hProcess,
            pid: process.dwProcessId,
        };
        if let Err(e) = adopt(child.process, thread.0) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SpawnError::Tie(e));
        }
        drop(thread);

        // SAFETY: each handle is ours, open, and handed to exactly one File.
        let file = |h: Owned| unsafe { File::from_raw_handle(h.take().0 as RawHandle) };
        Ok(Spawned {
            child,
            stdin: file(stdin_ours),
            stdout: Box::new(file(stdout_ours)),
            stderr: Box::new(file(stderr_ours)),
        })
    }

    /// The shell's environment with `env` applied (a key matching case-insensitively replaces
    /// the shell's), as a sorted UTF-16 block.
    fn environment(env: &[(OsString, OsString)]) -> Vec<u16> {
        let mut vars: BTreeMap<Vec<u16>, (OsString, OsString)> = BTreeMap::new();
        let key = |k: &OsStr| -> Vec<u16> {
            k.encode_wide()
                .map(|c| {
                    if (b'a' as u16..=b'z' as u16).contains(&c) {
                        c - 32
                    } else {
                        c
                    }
                })
                .collect()
        };
        for (k, v) in std::env::vars_os().chain(env.iter().cloned()) {
            vars.insert(key(&k), (k, v));
        }
        let mut block = Vec::new();
        for (k, v) in vars.values() {
            block.extend(k.encode_wide());
            block.push(b'=' as u16);
            block.extend(v.encode_wide());
            block.push(0);
        }
        block.push(0);
        block
    }

    /// The program to run. A bare name (the dev worker's `php`) is looked up on the worker's own
    /// PATH, with `.exe` when it has no extension, as std does; never in the current directory.
    fn resolve(program: &OsStr, cwd: &Path, block: &[u16]) -> io::Result<PathBuf> {
        let path = Path::new(program);
        if path.components().count() > 1 || path.is_absolute() {
            return Ok(if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            });
        }
        let names: Vec<PathBuf> = if path.extension().is_some() {
            vec![path.to_path_buf()]
        } else {
            vec![path.with_extension("exe"), path.to_path_buf()]
        };
        let paths = path_var(block).unwrap_or_default();
        for dir in std::env::split_paths(&paths) {
            for name in &names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "program not found on PATH",
        ))
    }

    /// PATH from an environment block.
    fn path_var(block: &[u16]) -> Option<OsString> {
        block.split(|&c| c == 0).find_map(|entry| {
            let eq = entry.iter().position(|&c| c == b'=' as u16)?;
            let name = String::from_utf16_lossy(&entry[..eq]);
            name.eq_ignore_ascii_case("PATH")
                .then(|| OsString::from_wide(&entry[eq + 1..]))
        })
    }

    /// The worker's Job Object (grafida/grafida#91). One per shell, created on first use and never
    /// closed: the system closes it when the shell's process ends, however it ends, and
    /// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` then kills the worker and every process it started.
    ///
    /// The shell itself is deliberately **not** in the job: a browser `ShellExecute` starts for
    /// `_native/open-url` would be in it too, and die with the app.
    struct Job(HANDLE);
    // SAFETY: a job handle is a kernel object handle, usable from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    static JOB: OnceLock<Result<Job, String>> = OnceLock::new();

    fn job() -> io::Result<HANDLE> {
        let job = JOB.get_or_init(|| {
            // SAFETY: plain Win32 calls; the limit structure outlives the call that reads it.
            unsafe {
                let h = CreateJobObjectW(None, None).map_err(|e| e.message())?;
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
                .map_err(|e| e.message())?;
                Ok(Job(h))
            }
        });
        match job {
            Ok(j) => Ok(j.0),
            Err(e) => Err(io::Error::other(e.clone())),
        }
    }

    /// The suspended worker goes in the job, then its one thread runs.
    fn adopt(process: HANDLE, thread: HANDLE) -> io::Result<()> {
        let job = job()?;
        // SAFETY: both handles are live; the process has not run an instruction yet.
        unsafe {
            AssignProcessToJobObject(job, process).map_err(|e| {
                io::Error::other(format!("AssignProcessToJobObject: {}", e.message()))
            })?;
            if ResumeThread(thread) == u32::MAX {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::append_arg;

    fn quoted(arg: &str, force: bool) -> String {
        let mut line = Vec::new();
        append_arg(&mut line, &arg.encode_utf16().collect::<Vec<_>>(), force);
        String::from_utf16(&line).unwrap()
    }

    #[test]
    fn arguments_are_quoted_as_the_msvc_runtime_reads_them() {
        assert_eq!(quoted("bin/worker.php", false), "bin/worker.php");
        assert_eq!(quoted("", false), "\"\"");
        assert_eq!(quoted("two words", false), "\"two words\"");
        assert_eq!(quoted("tab\there", false), "\"tab\there\"");
        assert_eq!(quoted(r#"say "hi""#, false), r#""say \"hi\"""#);
        assert_eq!(quoted(r"C:\a b\", false), r#""C:\a b\\""#);
        assert_eq!(quoted(r#"a\"b"#, false), r#"a\\\"b"#);
        assert_eq!(
            quoted(r"C:\Program Files\php\php.exe", true),
            r#""C:\Program Files\php\php.exe""#
        );
        assert_eq!(quoted(r"C:\no\spaces", true), r#""C:\no\spaces""#);
    }
}
