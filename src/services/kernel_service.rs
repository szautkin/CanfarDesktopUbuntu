//! Local Python kernel service for Verbinal notebook execution.
//!
//! Spawns a Python subprocess running `kernel_harness.py` and communicates
//! with it over stdin/stdout using a line-delimited JSON protocol.
//!
//! # Protocol
//!
//! Requests (written to the child's stdin, one JSON line each):
//! ```json
//! {"type": "execute", "code": "...", "exec_count": N}
//! {"type": "quit"}
//! ```
//!
//! Responses (read from the child's stdout):
//! Zero or more JSON output lines, terminated by the boundary sentinel:
//! ```text
//! \x04__CANFAR_EXEC_BOUNDARY__\x04
//! ```

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::{timeout, Duration};

/// Embedded Python harness script.  The path is relative to this source file:
/// `src/services/kernel_service.rs` → `../../data/kernel_harness.py`.
const KERNEL_HARNESS: &str = include_str!("../../data/kernel_harness.py");

/// Sentinel that the harness writes after every cell execution.
const BOUNDARY: &str = "\x04__CANFAR_EXEC_BOUNDARY__\x04";

/// Grace period given to the Python process after sending `quit` before we
/// issue a hard kill.
const SHUTDOWN_GRACE_SECS: u64 = 2;

// ── state ──────────────────────────────────────────────────────────────────

/// Observable lifecycle state of the kernel subprocess.
#[derive(Debug, Clone, PartialEq)]
pub enum KernelState {
    /// No process has been started yet (or it was cleaned up after shutdown).
    Dead,
    /// The process is being launched; I/O channels not yet ready.
    Starting,
    /// Ready and waiting for a cell.
    Idle,
    /// Currently executing a cell.
    Busy,
    /// The process died unexpectedly or an I/O error occurred.
    Error(String),
}

// ── service ────────────────────────────────────────────────────────────────

/// Manages a single local Python kernel subprocess.
///
/// Call [`start`](LocalKernelService::start) before [`execute`](LocalKernelService::execute).
/// The service is intentionally single-threaded from the caller's perspective:
/// only one `execute` should be in flight at a time.  Callers must enforce
/// this (e.g. by checking `state() == KernelState::Idle` before sending).
pub struct LocalKernelService {
    state: KernelState,
    process: Option<Child>,
    stdin: Option<BufWriter<ChildStdin>>,
    reader: Option<BufReader<ChildStdout>>,
    /// Path to the Python interpreter to use (e.g. `/usr/bin/python3`).
    python_path: PathBuf,
    exec_count: u32,
    /// Path of the temporary harness `.py` file written at `start()`.
    /// Removed on `cleanup_process` / `Drop`.
    harness_path: Option<PathBuf>,
    /// The kernel's stderr, read only when it dies unexpectedly — that is the
    /// one place Python explains why it could not start.
    stderr: Option<BufReader<tokio::process::ChildStderr>>,
    /// Where to forward each output as it arrives, so the UI can render a long
    /// cell's stdout live instead of showing nothing until the cell finishes.
    /// Set for the duration of one `execute_streaming` call.
    output_sink: Option<tokio::sync::mpsc::UnboundedSender<serde_json::Value>>,
}

impl LocalKernelService {
    /// Create a new, unstarted kernel service.
    ///
    /// `python_path` should be an absolute path to a Python 3 interpreter,
    /// e.g. `PathBuf::from("/usr/bin/python3")`.
    pub fn new(python_path: PathBuf) -> Self {
        Self {
            state: KernelState::Dead,
            process: None,
            stdin: None,
            reader: None,
            python_path,
            exec_count: 0,
            harness_path: None,
            stderr: None,
            output_sink: None,
        }
    }

    // ── public API ──────────────────────────────────────────────────────────

    /// Launch the kernel subprocess.
    ///
    /// 1. Writes `KERNEL_HARNESS` to a temporary file.
    /// 2. Spawns `python -u <harness_path>` with piped stdin/stdout.
    /// 3. Transitions the state to [`KernelState::Idle`].
    ///
    /// Returns `Err` if the file could not be written or the process could
    /// not be spawned.  Safe to call again after a previous `start` if the
    /// kernel is in [`KernelState::Dead`] or [`KernelState::Error`].
    pub async fn start(&mut self) -> Result<(), String> {
        // Clean up any remnants of a previous run.
        self.cleanup_process();

        self.state = KernelState::Starting;

        // Write the harness to a temp file so Python can load it.
        let harness_path = Self::write_harness_to_temp()
            .map_err(|e| format!("Failed to write kernel harness: {e}"))?;

        // Spawn: `-u` disables stdout/stderr buffering in Python.
        let mut child = Command::new(&self.python_path)
            .arg("-u")
            .arg(&harness_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            // Capture stderr rather than discarding it. The harness reports cell
            // errors as JSON on stdout, but anything that kills Python BEFORE the
            // harness runs — a missing interpreter dependency, an unreadable
            // harness, an import failure — is only ever explained on stderr. With
            // it discarded the user just saw "exited unexpectedly" and had nothing
            // to act on.
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                format!(
                    "Failed to spawn Python kernel at {:?}: {e}",
                    self.python_path
                )
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Could not capture kernel stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Could not capture kernel stdout".to_string())?;
        let stderr = child.stderr.take();

        self.stdin = Some(BufWriter::new(stdin));
        self.reader = Some(BufReader::new(stdout));
        self.process = Some(child);
        self.stderr = stderr.map(BufReader::new);
        self.harness_path = Some(harness_path);
        self.exec_count = 0;
        self.state = KernelState::Idle;

        Ok(())
    }

    /// Send `code` to the kernel for execution and collect all output lines.
    ///
    /// Blocks (asynchronously) until the harness emits the boundary sentinel
    /// or the child dies. There is deliberately **no** fatal execution
    /// deadline: a long-running cell must never hard-kill the kernel. The UI
    /// layer surfaces a soft, configurable timeout *warning*
    /// (`NotebookSettings.execution_timeout_secs`) and offers Interrupt
    /// (SIGINT) while execution continues.
    ///
    /// Returns `(outputs, execution_count)` where `outputs` is a `Vec` of
    /// raw `serde_json::Value` objects — one per output line emitted by the
    /// harness.
    ///
    /// Transitions state: `Idle → Busy → Idle` on success, or
    /// `Idle → Busy → Error(…)` on genuine I/O failure / unexpected EOF.
    /// Like [`execute`](Self::execute), but forwards every output through `sink`
    /// as the kernel emits it.
    ///
    /// The complete output list is still returned, so the caller stores exactly
    /// what it would have stored otherwise; the sink is purely for showing
    /// progress. The sink is cleared before returning — including on the error
    /// path — so a later run can never publish into a closed channel.
    pub async fn execute_streaming(
        &mut self,
        code: &str,
        sink: tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
    ) -> Result<(Vec<serde_json::Value>, u32), String> {
        self.output_sink = Some(sink);
        let result = self.execute(code).await;
        self.output_sink = None;
        result
    }

    pub async fn execute(&mut self, code: &str) -> Result<(Vec<serde_json::Value>, u32), String> {
        if self.state != KernelState::Idle {
            return Err(format!(
                "Kernel is not idle (current state: {:?})",
                self.state
            ));
        }
        self.state = KernelState::Busy;
        self.exec_count += 1;
        let current_count = self.exec_count;

        // Build the request JSON.
        let request = serde_json::json!({
            "type": "execute",
            "code": code,
            "exec_count": current_count,
        });
        let request_line = format!("{}\n", request);

        // Write the request.
        let write_result = self.write_line(&request_line).await;
        if let Err(e) = write_result {
            self.state = KernelState::Error(e.clone());
            return Err(e);
        }

        // Read output lines until we see the boundary sentinel. There is
        // deliberately NO fatal execution deadline here: a long-running cell
        // must never hard-kill the kernel or force it into `Error`. The UI
        // layer surfaces a soft, configurable timeout warning
        // (`NotebookSettings.execution_timeout_secs`) and offers Interrupt
        // (SIGINT) while we keep waiting for the real result. Genuine kernel
        // death is still detected via EOF / I/O error inside
        // `read_until_boundary`. Mirrors the reference
        // `LocalKernelService.ExecuteAsync`, which has no execution timeout.
        let read_result = self.read_until_boundary().await;

        match read_result {
            Ok(outputs) => {
                self.state = KernelState::Idle;
                Ok((outputs, current_count))
            }
            Err(e) => {
                self.state = KernelState::Error(e.clone());
                Err(e)
            }
        }
    }

    /// Send `SIGINT` to the kernel process to interrupt a running cell.
    ///
    /// The harness catches `KeyboardInterrupt` and emits an error output,
    /// then returns to the idle loop — so the protocol is not broken.
    ///
    /// No-op if the process is not running or has no PID.
    pub fn interrupt(&mut self) {
        if let Some(child) = &self.process {
            if let Some(pid) = child.id() {
                // SAFETY: `kill` with SIGINT is safe to call on any valid PID.
                // The worst outcome is ESRCH if the process already exited.
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGINT);
                }
            }
        }
    }

    /// Kill the current kernel and start a fresh one.
    ///
    /// The new kernel inherits the same `python_path` but starts with a
    /// clean execution namespace and `exec_count` reset to 0.
    pub async fn restart(&mut self) -> Result<(), String> {
        self.shutdown();
        self.start().await
    }

    /// Gracefully stop the kernel.
    ///
    /// Sends `{"type": "quit"}` to the harness, waits up to
    /// [`SHUTDOWN_GRACE_SECS`] for a clean exit, then kills the process if it
    /// is still running.
    ///
    /// After this call the state is [`KernelState::Dead`].
    pub fn shutdown(&mut self) {
        // Best-effort quit message — ignore errors; we kill regardless.
        if self.stdin.is_some() {
            // Use a blocking write because `shutdown` takes `&mut self` (not async).
            // We drop the writer to close stdin, which signals the harness to exit.
            if let Some(mut stdin) = self.stdin.take() {
                // Attempt a non-async write via the underlying handle.
                // `BufWriter` wraps `ChildStdin` which is a `tokio::process::ChildStdin`.
                // We cannot call `.await` here, so we spawn a fire-and-forget task.
                let quit_line = "{\"type\":\"quit\"}\n".to_string();
                tokio::spawn(async move {
                    let _ = stdin.write_all(quit_line.as_bytes()).await;
                    let _ = stdin.flush().await;
                    // Drop stdin → EOF → harness loop exits.
                });
            }
        }

        // Drop I/O handles; the child will get EOF and exit naturally.
        self.reader = None;

        // Give the process a moment, then kill it.
        if let Some(mut child) = self.process.take() {
            // Spawn a small task that waits for the grace period then kills.
            tokio::spawn(async move {
                let wait = timeout(Duration::from_secs(SHUTDOWN_GRACE_SECS), child.wait());
                if let Ok(Ok(_)) = wait.await {
                    // Process exited cleanly.
                } else {
                    // Timeout or error — force kill.
                    let _ = child.kill().await;
                }
            });
        }

        if let Some(path) = self.harness_path.take() {
            let _ = std::fs::remove_file(&path);
        }
        self.state = KernelState::Dead;
    }

    /// Current lifecycle state of the kernel.
    pub fn state(&self) -> &KernelState {
        &self.state
    }

    /// Number of cells executed since the last `start` / `restart`.
    pub fn exec_count(&self) -> u32 {
        self.exec_count
    }

    // ── private helpers ─────────────────────────────────────────────────────

    /// Write `KERNEL_HARNESS` to a temporary file and return its path.
    ///
    /// Uses `std::env::temp_dir()` with a name derived from the current
    /// process ID and a monotonic timestamp to avoid collisions.
    /// The caller is responsible for deleting the file when done.
    /// Read whatever the dead kernel left on stderr, trimmed to something a user
    /// can read.
    ///
    /// Only called after EOF on stdout, so the child is already gone and this
    /// cannot block on a live process. A long Python traceback is truncated to
    /// its tail, which is where the actual cause is.
    async fn drain_stderr(&mut self) -> Option<String> {
        use tokio::io::AsyncReadExt;
        let mut reader = self.stderr.take()?;
        let mut buf = String::new();
        reader.read_to_string(&mut buf).await.ok()?;
        let text = buf.trim();
        if text.is_empty() {
            return None;
        }
        const MAX: usize = 600;
        if text.len() <= MAX {
            return Some(text.to_string());
        }
        // Keep the TAIL: a traceback names the real error on its last lines.
        let tail: String = text
            .chars()
            .skip(text.chars().count().saturating_sub(MAX))
            .collect();
        Some(format!("…{tail}"))
    }

    fn write_harness_to_temp() -> std::io::Result<PathBuf> {
        use std::io::Write;

        // Build a collision-resistant filename without external crates.
        let pid = std::process::id();
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let filename = format!("verbinal_kernel_harness_{pid}_{ts}.py");
        let path = std::env::temp_dir().join(filename);

        let mut f = std::fs::File::create(&path)?;
        f.write_all(KERNEL_HARNESS.as_bytes())?;
        f.flush()?;
        Ok(path)
    }

    /// Write a single line to the child's stdin and flush.
    async fn write_line(&mut self, line: &str) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "Kernel stdin not available".to_string())?;

        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("Failed to write to kernel stdin: {e}"))?;

        stdin
            .flush()
            .await
            .map_err(|e| format!("Failed to flush kernel stdin: {e}"))?;

        Ok(())
    }

    /// Read stdout lines until the boundary sentinel is encountered.
    ///
    /// Returns the parsed JSON values of all non-sentinel lines.
    async fn read_until_boundary(&mut self) -> Result<Vec<serde_json::Value>, String> {
        let reader = self
            .reader
            .as_mut()
            .ok_or_else(|| "Kernel stdout not available".to_string())?;

        let mut outputs: Vec<serde_json::Value> = Vec::new();
        let mut line_buf = String::new();

        loop {
            line_buf.clear();
            let n = reader
                .read_line(&mut line_buf)
                .await
                .map_err(|e| format!("Failed to read from kernel stdout: {e}"))?;

            if n == 0 {
                // EOF — the process exited unexpectedly. Whatever Python wrote to
                // stderr is the only diagnosis available, so surface it.
                let detail = self.drain_stderr().await;
                return Err(match detail {
                    Some(msg) => format!("Kernel process exited unexpectedly: {msg}"),
                    None => "Kernel process exited unexpectedly (EOF on stdout)".to_string(),
                });
            }

            let trimmed = line_buf.trim_end_matches('\n').trim_end_matches('\r');

            if trimmed == BOUNDARY {
                break;
            }

            // Parse non-empty lines as JSON; silently skip blank lines.
            if trimmed.is_empty() {
                continue;
            }

            match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(v) => {
                    // Publish before storing, so the UI shows this line now
                    // rather than when the whole cell finishes. A closed
                    // receiver (the cell's widget went away mid-run) is not an
                    // error — the execution still completes and is recorded.
                    if let Some(sink) = self.output_sink.as_ref() {
                        let _ = sink.send(v.clone());
                    }
                    outputs.push(v);
                }
                Err(e) => {
                    // Emit a synthetic error output rather than aborting.
                    outputs.push(serde_json::json!({
                        "output_type": "error",
                        "ename": "HarnessProtocolError",
                        "evalue": format!("Failed to parse harness output as JSON: {e}"),
                        "traceback": [format!("Raw line: {:?}", trimmed)],
                    }));
                }
            }
        }

        Ok(outputs)
    }

    /// Drop all I/O handles and mark the process as gone without waiting for
    /// it to exit.  Used at the start of `start()` to clean up a previous run.
    fn cleanup_process(&mut self) {
        self.stdin = None;
        self.reader = None;
        // Dropping `Child` with `kill_on_drop(true)` will send SIGKILL.
        self.process = None;
        // Drop the old stderr too: keeping it would let a LATER kernel death
        // report the previous kernel's error message.
        self.stderr = None;
        // Remove the harness temp file; ignore errors (e.g. already gone).
        if let Some(path) = self.harness_path.take() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

impl Drop for LocalKernelService {
    fn drop(&mut self) {
        // Ensure the child process is killed when the service is dropped,
        // even if `shutdown` was never called.  We cannot await here, so we
        // rely on `kill_on_drop(true)` set at spawn time.
        self.stdin = None;
        self.reader = None;
        // `self.process` drop triggers the kill-on-drop behaviour.
        // Clean up the harness temp file.
        if let Some(path) = self.harness_path.take() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

// ── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that the embedded harness constant is non-empty and contains
    /// the expected boundary sentinel string, proving `include_str!` resolved
    /// correctly at compile time.
    #[test]
    fn harness_is_embedded() {
        assert!(
            !KERNEL_HARNESS.is_empty(),
            "KERNEL_HARNESS must not be empty"
        );
        assert!(
            KERNEL_HARNESS.contains("__CANFAR_EXEC_BOUNDARY__"),
            "Harness must define the boundary sentinel"
        );
        assert!(
            KERNEL_HARNESS.contains("def main()"),
            "Harness must define a main() function"
        );
    }

    /// Verify the boundary constant matches what the harness emits.
    #[test]
    fn boundary_constant_matches_harness() {
        // The Python harness defines: BOUNDARY = "\x04__CANFAR_EXEC_BOUNDARY__\x04"
        assert_eq!(BOUNDARY, "\x04__CANFAR_EXEC_BOUNDARY__\x04");
    }

    /// A freshly created service must be in the Dead state with exec_count 0.
    #[test]
    fn initial_state_is_dead() {
        let svc = LocalKernelService::new(PathBuf::from("/usr/bin/python3"));
        assert_eq!(*svc.state(), KernelState::Dead);
        assert_eq!(svc.exec_count(), 0);
    }

    /// Calling `execute` on a non-Idle kernel must return an error immediately.
    #[tokio::test]
    async fn execute_on_dead_kernel_returns_error() {
        let mut svc = LocalKernelService::new(PathBuf::from("/usr/bin/python3"));
        // State is Dead, not Idle.
        let result = svc.execute("1 + 1").await;
        assert!(result.is_err(), "execute on Dead kernel must fail");
    }

    /// The harness temp-file writer must succeed (it only needs a writable /tmp).
    /// A harness path must be unique per launch, or one notebook tab's cleanup
    /// deletes another tab's in-use file. (Each tab owns its own kernel, so this
    /// is the shape the reference's restart race took here — `&mut self` already
    /// serialises two starts on ONE service, but not across services.)
    #[test]
    fn harness_paths_are_unique_per_launch() {
        let a = LocalKernelService::write_harness_to_temp().unwrap();
        let b = LocalKernelService::write_harness_to_temp().unwrap();
        assert_ne!(a, b, "two launches must not share a harness file");
        assert!(a.exists() && b.exists());
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    /// A kernel that dies before the harness runs explains itself only on
    /// stderr; that used to be discarded, leaving the user with "exited
    /// unexpectedly" and nothing to act on.
    #[tokio::test]
    async fn an_unstartable_interpreter_reports_why() {
        // A path that is not an interpreter at all: spawn itself fails, which is
        // the error the user must see.
        let mut svc =
            LocalKernelService::new(PathBuf::from("/nonexistent/python-that-is-not-there"));
        let err = svc.start().await.expect_err("this cannot start");
        assert!(
            err.contains("Failed to spawn"),
            "the failure should name what went wrong: {err}"
        );
    }

    #[test]
    fn write_harness_to_temp_succeeds() {
        let path = LocalKernelService::write_harness_to_temp()
            .expect("write_harness_to_temp should succeed");
        assert!(path.exists(), "temp file must exist after creation");
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, KERNEL_HARNESS);
        // Clean up.
        let _ = std::fs::remove_file(&path);
    }

    /// Full round-trip: start the kernel, execute a trivial expression, shut
    /// down.  This test requires Python 3 at `/usr/bin/python3`.
    ///
    /// Marked `#[ignore]` so it does not run in CI environments that lack
    /// Python.  Run with `cargo test -- --ignored` when Python is available.
    #[tokio::test]
    #[ignore]
    async fn roundtrip_execute() {
        let mut svc = LocalKernelService::new(PathBuf::from("/usr/bin/python3"));
        svc.start().await.expect("kernel should start");
        assert_eq!(*svc.state(), KernelState::Idle);

        let (outputs, count) = svc.execute("1 + 1").await.expect("execute should succeed");
        assert_eq!(count, 1);
        // The expression `1 + 1` should produce one execute_result output.
        let result_output = outputs
            .iter()
            .find(|v| v["output_type"] == "execute_result")
            .expect("expected an execute_result output");
        assert_eq!(result_output["data"]["text/plain"], "2");

        svc.shutdown();
        assert_eq!(*svc.state(), KernelState::Dead);
    }

    /// Streaming must deliver each output as the kernel emits it AND still return
    /// the complete list — the sink is for showing progress, not a replacement
    /// for the stored result.
    ///
    /// Requires Python 3, so `#[ignore]` like its siblings; run with
    /// `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn streaming_publishes_each_output_and_still_returns_them_all() {
        let mut svc = LocalKernelService::new(PathBuf::from("/usr/bin/python3"));
        svc.start().await.expect("kernel should start");

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (outputs, _) = svc
            .execute_streaming("print('a'); print('b')", tx)
            .await
            .expect("execute should succeed");

        let mut streamed = Vec::new();
        while let Ok(v) = rx.try_recv() {
            streamed.push(v);
        }
        assert!(!streamed.is_empty(), "the sink received nothing");
        assert_eq!(
            streamed.len(),
            outputs.len(),
            "every returned output should also have been published"
        );

        svc.shutdown();
    }

    /// A closed receiver must not fail the execution: the cell's widget can go
    /// away mid-run (tab closed), and the run still has to complete and record.
    #[tokio::test]
    #[ignore]
    async fn a_dropped_output_receiver_does_not_fail_the_run() {
        let mut svc = LocalKernelService::new(PathBuf::from("/usr/bin/python3"));
        svc.start().await.expect("kernel should start");

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        let result = svc.execute_streaming("print('ignored')", tx).await;
        assert!(result.is_ok(), "a dropped receiver must not fail the run");

        svc.shutdown();
    }

    /// Verify that executing code with a deliberate error produces an error
    /// output rather than crashing the service.
    #[tokio::test]
    #[ignore]
    async fn error_output_on_exception() {
        let mut svc = LocalKernelService::new(PathBuf::from("/usr/bin/python3"));
        svc.start().await.expect("kernel should start");

        let (outputs, _) = svc
            .execute("raise ValueError('test error')")
            .await
            .expect("execute should succeed (harness catches exceptions)");

        let error_output = outputs
            .iter()
            .find(|v| v["output_type"] == "error")
            .expect("expected an error output");
        assert_eq!(error_output["ename"], "ValueError");
        assert!(error_output["evalue"]
            .as_str()
            .unwrap()
            .contains("test error"));

        // The kernel must be Idle again — ready for the next cell.
        assert_eq!(*svc.state(), KernelState::Idle);

        svc.shutdown();
    }
}
