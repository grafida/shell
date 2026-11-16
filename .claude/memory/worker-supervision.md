# Worker supervision and packaged-mode resolution

Read before changing the supervisor thread, worker spawning, restart handling, the fake worker, or how the worker is located.

- **The worker is spawned from the supervisor thread, and must stay so.** `PR_SET_PDEATHSIG` fires
  when the *thread* that forked the child exits, not the process. Spawn from a short-lived thread and
  the worker is SIGTERMed the moment that thread ends. The supervisor thread is one event loop over
  one channel (submissions, frames from a stdout-reader thread, end-of-stdout); the UI thread only
  submits and never waits. A crash answers the in-flight request `502 {"error":"worker_restarted"}`
  and respawns; one crash more than `restart.max` inside `restart.windowSeconds` is fatal, and so is
  any failure **before the first `ready`** (a worker that cannot start once will not start on retry).
  The shell gives the worker its body limit as `SHELL_MAX_BODY_BYTES`.
- **`tests/workers/fake_worker.py` is a third, independent implementation of the worker side**, for
  the misbehaviour the supervisor must survive (never ready, wrong protocol, garbage, crashes, ignoring
  EOF). Like the fixture writer, it must never share code with either codec. `tests/real_workers.rs`
  runs shell-php's echo worker and Grafida's `bin/worker.php` (sibling clones, or `SHELL_PHP_DIR` /
  `GRAFIDA_DIR`) and skips with a note when they are absent. Grafida's worker runs with every
  per-user directory pointed at a temp dir, never the real data.
- **In packaged mode the worker is resolved beside the shell's real path** (`cli.rs`), not beside the
  manifest: the same directory on Linux, `Contents/MacOS` in a bundle (codesign wants only code
  there); the manifest is beside the binary or in `../Resources`, and the worker runs in its directory.
