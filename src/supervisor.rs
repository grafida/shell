//! The worker supervisor: spawns the worker, waits for `ready`, feeds it one request at a time,
//! restarts it when it crashes, and gives up (with the tail of its stderr) when it keeps crashing.
//!
//! Everything happens on one dedicated **supervisor thread**, driven by a single event channel.
//! Two reader threads per worker process feed that channel: one decodes frames from stdout, one
//! drains stderr into the log and a ring buffer of recent lines. The UI thread only ever *submits*
//! a request with a responder and returns at once. It never waits on the worker.
//!
//! ⚠️ The worker is spawned **from the supervisor thread**, and must stay so. On Linux the child
//! asks for `PR_SET_PDEATHSIG`, which fires when the *thread* that forked it exits, not the
//! process. A worker spawned from a short-lived thread would be killed as soon as that thread
//! ended. The supervisor thread lives until shutdown, when the worker is stopped anyway.

use std::{
    collections::VecDeque,
    ffi::OsString,
    io::{self, BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    http::{Request, Response},
    protocol::{self, Frame, Header, PROTOCOL_VERSION},
};

/// Called exactly once with the answer to a submitted request, on the supervisor thread. The
/// platform layer posts it to the UI thread from there.
pub type Responder = Box<dyn FnOnce(Response) + Send>;

/// Receives one line of log text: `(source, line)`, e.g. `("worker", "[stray-output] …")`.
pub type LogFn = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// How long a stopping worker may take after EOF before it is killed.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// How many lines of worker stderr the fatal alert shows.
pub const STDERR_TAIL_LINES: usize = 40;

/// The longest stderr line kept; anything beyond is cut, so a runaway line cannot exhaust memory.
const MAX_STDERR_LINE: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub struct WorkerCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    /// Extra environment variables for the worker, on top of the shell's own.
    pub env: Vec<(OsString, OsString)>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub command: WorkerCommand,
    pub ready_timeout: Duration,
    pub max_body_bytes: u64,
    /// Restarts allowed within `restart_window`; one crash more is fatal.
    pub restart_max: u32,
    pub restart_window: Duration,
    pub shutdown_grace: Duration,
}

/// What the worker said about itself in `ready`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyInfo {
    pub pid: u64,
    pub app: Option<String>,
    pub version: Option<String>,
}

/// Why the supervisor gave up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub reason: String,
    pub stderr_tail: Vec<String>,
}

/// What the supervisor tells the platform layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// The worker is ready. `first` is true once per supervisor: the moment to load the page.
    Ready { info: ReadyInfo, first: bool },
    /// The worker died (or never became ready) and is being restarted.
    Restarting { reason: String },
    /// The supervisor has given up; show `Failure` and quit. Requests answer 503 from now on.
    Fatal(Failure),
}

pub type NoticeFn = Box<dyn Fn(Notice) + Send>;

enum Event {
    Submit(Request, Responder),
    Frame(u64, Frame),
    /// The worker's stdout ended: `None` at a clean EOF, otherwise the protocol or I/O error.
    StdoutEnded(u64, Option<String>),
    Shutdown,
}

/// A handle on the supervisor thread. Dropping it shuts the worker down.
pub struct Supervisor {
    events: Sender<Event>,
    thread: Mutex<Option<JoinHandle<()>>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

impl Supervisor {
    /// Starts the supervisor thread, which spawns the worker and waits for it to be ready.
    pub fn start(config: Config, notify: NoticeFn, log: LogFn) -> io::Result<Supervisor> {
        let (events, inbox) = mpsc::channel();
        let stderr = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
        let state = State {
            config,
            notify,
            log,
            events: events.clone(),
            stderr: stderr.clone(),
            worker: None,
            generation: 0,
            ever_ready: false,
            fatal: false,
            next_id: 1,
            queue: VecDeque::new(),
            in_flight: None,
            crashes: VecDeque::new(),
        };
        let thread = thread::Builder::new()
            .name("worker-supervisor".into())
            .spawn(move || state.run(inbox))?;

        Ok(Supervisor {
            events,
            thread: Mutex::new(Some(thread)),
            stderr,
        })
    }

    /// Queues a request. The responder is always called, eventually, unless the supervisor has
    /// already been shut down.
    pub fn submit(&self, request: Request, responder: Responder) {
        if let Err(mpsc::SendError(Event::Submit(_, responder))) =
            self.events.send(Event::Submit(request, responder))
        {
            responder(Response::error(503, "worker_unavailable"));
        }
    }

    /// The last lines the worker wrote to stderr.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr.lock().unwrap().iter().cloned().collect()
    }

    /// Stops the worker gracefully (EOF, then up to the grace period, then kill) and waits for
    /// it. Idempotent; requests submitted afterwards are answered 503.
    pub fn shutdown(&self) {
        let thread = self.thread.lock().unwrap().take();
        if let Some(thread) = thread {
            let _ = self.events.send(Event::Shutdown);
            let _ = thread.join();
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Running {
    child: Child,
    stdin: Option<ChildStdin>,
    ready: bool,
    deadline: Instant,
}

struct State {
    config: Config,
    notify: NoticeFn,
    log: LogFn,
    events: Sender<Event>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    worker: Option<Running>,
    generation: u64,
    ever_ready: bool,
    fatal: bool,
    next_id: u64,
    queue: VecDeque<(Request, Responder)>,
    in_flight: Option<(u64, Responder)>,
    crashes: VecDeque<Instant>,
}

impl State {
    fn run(mut self, inbox: Receiver<Event>) {
        self.spawn();

        loop {
            let event = match self.ready_deadline() {
                Some(deadline) => {
                    match inbox.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Ok(event) => event,
                        Err(RecvTimeoutError::Timeout) => {
                            let secs = self.config.ready_timeout.as_secs_f64();
                            self.crashed(format!(
                                "the worker did not send `ready` within {secs:.0} s"
                            ));
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match inbox.recv() {
                    Ok(event) => event,
                    Err(_) => break,
                },
            };

            match event {
                Event::Submit(request, responder) => {
                    if self.fatal {
                        responder(Response::error(503, "worker_unavailable"));
                    } else {
                        self.queue.push_back((request, responder));
                        self.dispatch();
                    }
                }
                Event::Frame(generation, frame) if generation == self.generation => {
                    self.frame(frame)
                }
                Event::StdoutEnded(generation, error) if generation == self.generation => {
                    let reason = match error {
                        None => "the worker exited".to_string(),
                        Some(e) => format!("the worker broke the protocol ({e})"),
                    };
                    self.crashed(reason);
                }
                Event::Frame(..) | Event::StdoutEnded(..) => {} // from a worker already replaced
                Event::Shutdown => break,
            }
        }

        self.stop_worker();
        self.fail_all("worker_unavailable", 503);
    }

    fn ready_deadline(&self) -> Option<Instant> {
        match &self.worker {
            Some(w) if !w.ready => Some(w.deadline),
            _ => None,
        }
    }

    fn spawn(&mut self) {
        self.generation += 1;
        let generation = self.generation;
        let command = &self.config.command;

        let mut cmd = Command::new(&command.program);
        cmd.args(&command.args)
            .current_dir(&command.cwd)
            .envs(command.env.iter().map(|(k, v)| (k, v)))
            .env(
                "SHELL_MAX_BODY_BYTES",
                self.config.max_body_bytes.to_string(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        tie_to_parent(&mut cmd);

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                let program = command.program.to_string_lossy().into_owned();
                // Nothing to wait for: report it straight away, through the normal crash path.
                self.worker = None;
                self.crashed(format!("cannot start the worker `{program}`: {e}"));
                return;
            }
        };

        (self.log)("shell", &format!("worker started, pid {}", child.id()));

        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let events = self.events.clone();
        let max_body = self.config.max_body_bytes;
        thread::Builder::new()
            .name("worker-stdout".into())
            .spawn(move || read_stdout(stdout, generation, max_body, events))
            .expect("spawn the stdout reader");

        let ring = self.stderr.clone();
        let log = self.log.clone();
        thread::Builder::new()
            .name("worker-stderr".into())
            .spawn(move || read_stderr(stderr, ring, log))
            .expect("spawn the stderr reader");

        self.worker = Some(Running {
            stdin: child.stdin.take(),
            child,
            ready: false,
            deadline: Instant::now() + self.config.ready_timeout,
        });
    }

    fn frame(&mut self, frame: Frame) {
        let ready = self.worker.as_ref().is_some_and(|w| w.ready);

        match (ready, frame.header) {
            (
                false,
                Header::Ready {
                    protocol,
                    pid,
                    app,
                    version,
                },
            ) => {
                if protocol != u64::from(PROTOCOL_VERSION) {
                    self.give_up(format!(
                        "the worker speaks protocol {protocol}, but this shell speaks {PROTOCOL_VERSION}"
                    ));
                    return;
                }
                if let Some(w) = self.worker.as_mut() {
                    w.ready = true;
                }
                let first = !self.ever_ready;
                self.ever_ready = true;
                let info = ReadyInfo { pid, app, version };
                (self.log)(
                    "shell",
                    &format!(
                        "worker ready: pid {} {} {}",
                        info.pid,
                        info.app.as_deref().unwrap_or("-"),
                        info.version.as_deref().unwrap_or("-")
                    ),
                );
                (self.notify)(Notice::Ready { info, first });
                self.dispatch();
            }
            (
                true,
                Header::Response {
                    id,
                    status,
                    headers,
                },
            ) if self.in_flight.as_ref().is_some_and(|(want, _)| *want == id) => {
                let (_, responder) = self.in_flight.take().unwrap();
                responder(Response {
                    status,
                    headers,
                    body: frame.body,
                });
                self.dispatch();
            }
            (_, header) => {
                let what = match header {
                    Header::Ready { .. } => "a second `ready`".to_string(),
                    Header::Response { id, .. } => {
                        format!("a response to request {id}, which was not asked")
                    }
                    Header::Request { .. } => "a `request` frame".to_string(),
                    Header::Shutdown => "`shutdown` (it is exiting on its own)".to_string(),
                };
                let reason = if ready {
                    format!("the worker sent {what}")
                } else {
                    format!("the worker sent {what} before `ready`")
                };
                self.crashed(reason);
            }
        }
    }

    /// Sends the next queued request when the worker is ready and idle.
    fn dispatch(&mut self) {
        if self.in_flight.is_some() || !self.worker.as_ref().is_some_and(|w| w.ready) {
            return;
        }
        let Some((request, responder)) = self.queue.pop_front() else {
            return;
        };

        let id = self.next_id;
        self.next_id += 1;
        let frame = Frame {
            header: Header::Request {
                id,
                method: request.method,
                url: request.url,
                headers: request.headers,
            },
            body: request.body,
        };

        let bytes = match protocol::encode_frame(&frame) {
            Ok(bytes) => bytes,
            Err(e) => {
                // The request itself is unencodable (e.g. a header value the protocol forbids).
                // That is the page's problem, not the worker's.
                (self.log)("shell", &format!("refused request {id}: {e}"));
                responder(Response::error(400, "bad_request"));
                self.dispatch();
                return;
            }
        };

        self.in_flight = Some((id, responder));

        let written = match self.worker.as_mut().and_then(|w| w.stdin.as_mut()) {
            Some(stdin) => stdin.write_all(&bytes).and_then(|()| stdin.flush()),
            None => Err(io::Error::from(io::ErrorKind::BrokenPipe)),
        };
        if let Err(e) = written {
            self.crashed(format!("cannot write to the worker: {e}"));
        }
    }

    /// The worker died, broke the protocol, or never became ready. Answer what was in flight, then
    /// restart it, unless it has crashed too often or never managed to start at all.
    fn crashed(&mut self, reason: String) {
        let status = self.stop_worker_now();
        let reason = match status {
            Some(status) => format!("{reason} ({})", describe(status)),
            None => reason,
        };
        (self.log)("shell", &format!("worker crashed: {reason}"));

        if let Some((_, responder)) = self.in_flight.take() {
            responder(Response::error(502, "worker_restarted"));
        }

        if !self.ever_ready {
            self.give_up(reason);
            return;
        }

        let now = Instant::now();
        self.crashes.push_back(now);
        while self
            .crashes
            .front()
            .is_some_and(|t| now.duration_since(*t) > self.config.restart_window)
        {
            self.crashes.pop_front();
        }

        if self.crashes.len() > self.config.restart_max as usize {
            let n = self.crashes.len();
            let secs = self.config.restart_window.as_secs();
            self.give_up(format!(
                "the worker crashed {n} times within {secs} s; the last time: {reason}"
            ));
            return;
        }

        self.push_stderr("──── worker restarted ────".to_string());
        (self.notify)(Notice::Restarting {
            reason: reason.clone(),
        });
        self.spawn();
    }

    fn give_up(&mut self, reason: String) {
        self.stop_worker_now();
        self.fatal = true;
        self.fail_all("worker_unavailable", 503);
        (self.log)("shell", &format!("giving up on the worker: {reason}"));
        let stderr_tail = self.stderr.lock().unwrap().iter().cloned().collect();
        (self.notify)(Notice::Fatal(Failure {
            reason,
            stderr_tail,
        }));
    }

    fn fail_all(&mut self, code: &str, status: u16) {
        if let Some((_, responder)) = self.in_flight.take() {
            responder(Response::error(status, code));
        }
        for (_, responder) in self.queue.drain(..) {
            responder(Response::error(status, code));
        }
    }

    fn push_stderr(&self, line: String) {
        push_line(&self.stderr, line);
    }

    /// Kills the worker at once and reaps it.
    fn stop_worker_now(&mut self) -> Option<ExitStatus> {
        let mut w = self.worker.take()?;
        drop(w.stdin.take());
        if let Ok(Some(status)) = w.child.try_wait() {
            return Some(status);
        }
        let _ = w.child.kill();
        w.child.wait().ok()
    }

    /// Graceful stop: `shutdown`, EOF, up to the grace period, then kill.
    fn stop_worker(&mut self) {
        let Some(mut w) = self.worker.take() else {
            return;
        };
        if let Some(mut stdin) = w.stdin.take() {
            if w.ready
                && self.in_flight.is_none()
                && let Ok(bytes) = protocol::encode_frame(&Frame {
                    header: Header::Shutdown,
                    body: Vec::new(),
                })
            {
                let _ = stdin.write_all(&bytes);
            }
            drop(stdin);
        }

        let deadline = Instant::now() + self.config.shutdown_grace;
        loop {
            match w.child.try_wait() {
                Ok(Some(status)) => {
                    (self.log)("shell", &format!("worker stopped ({})", describe(status)));
                    return;
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => break,
            }
        }
        let _ = w.child.kill();
        let _ = w.child.wait();
        (self.log)(
            "shell",
            &format!(
                "worker killed: still running {} ms after EOF",
                self.config.shutdown_grace.as_millis()
            ),
        );
    }
}

fn describe(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("killed by signal {signal}");
        }
    }
    match status.code() {
        Some(code) => format!("exit status {code}"),
        None => "no exit status".to_string(),
    }
}

fn read_stdout(stdout: impl Read, generation: u64, max_body: u64, events: Sender<Event>) {
    let mut reader = BufReader::with_capacity(64 * 1024, stdout);
    loop {
        match protocol::read_frame(&mut reader, max_body) {
            Ok(Some(frame)) => {
                if events.send(Event::Frame(generation, frame)).is_err() {
                    return;
                }
            }
            Ok(None) => {
                let _ = events.send(Event::StdoutEnded(generation, None));
                return;
            }
            Err(e) => {
                let _ = events.send(Event::StdoutEnded(generation, Some(e.to_string())));
                return;
            }
        }
    }
}

fn read_stderr(stderr: impl Read, ring: Arc<Mutex<VecDeque<String>>>, log: LogFn) {
    let mut reader = BufReader::new(stderr);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        // Bounded: a line longer than the cap is consumed in pieces, each logged on its own.
        let mut limited = (&mut reader).take(MAX_STDERR_LINE as u64);
        match limited.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf);
                let line = text.trim_end_matches(['\n', '\r']).to_string();
                log("worker", &line);
                push_line(&ring, line);
            }
        }
    }
}

fn push_line(ring: &Mutex<VecDeque<String>>, line: String) {
    let mut ring = ring.lock().unwrap();
    if ring.len() == STDERR_TAIL_LINES {
        ring.pop_front();
    }
    ring.push_back(line);
}

/// The worker must never outlive the shell.
#[cfg(target_os = "linux")]
fn tie_to_parent(cmd: &mut Command) {
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

#[cfg(not(target_os = "linux"))]
fn tie_to_parent(_cmd: &mut Command) {
    // macOS: EOF on stdin plus the worker's own parent watchdog; Windows: a Job Object
    // (grafida/grafida#82, "Lifetime coupling"). Both arrive with their platforms.
}
