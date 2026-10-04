//! Sandboxed decode: untrusted decoding in a separate process with a hard
//! memory ceiling.
//!
//! # Why this exists
//!
//! [`crate::validate::Limits`] is the first line of defence: it reads a header and
//! refuses to allocate for a 60000x60000 claim. It is not the whole defence,
//! because it is enforced *by the same code it is protecting*. A bug in a
//! decoder, or a format whose header lies in a way the checks do not model, has
//! no limit on how much memory it can take before the check runs. On a phone
//! that is an OOM kill of the whole app, including the UI the user was looking
//! at.
//!
//! So decoding also happens in a child process with an address-space limit the
//! **parent** sets. When the limit is hit, the child dies and the parent still has
//! its address space. That is the difference between "we checked the header" and
//! "we contained the failure".
//!
//! # Why a process and not a thread
//!
//! Threads share an address space, so an allocation failure in a worker thread
//! aborts the process. There is no way to cap a thread's memory in Rust, and
//! `setrlimit` is per-process. A thread here would be the same containment as no
//! containment at all.
//!
//! # Why the limit is set from outside
//!
//! `RLIMIT_AS` is applied in `pre_exec`, which runs in the child between `fork`
//! and `exec`, so the limit is in force before any of the engine's code runs. The
//! worker cannot raise it: a soft limit can be raised up to the hard limit, so the
//! parent sets **both** to the same value. A worker that calls `setrlimit` to make
//! itself comfortable gets `EINVAL`, and
//! `a_worker_cannot_raise_the_limit_it_was_given` proves it.
//!
//! A limit the worker set for itself would be a limit a compromised worker could
//! ignore, which is the entire thing this module exists to prevent.
//!
//! # Exit-code contract
//!
//! The two failure modes are different sentences for a user, so they are different
//! exit codes rather than one generic failure:
//!
//! | Code | Meaning | User-facing message |
//! | --- | --- | --- |
//! | 0 | success | — |
//! | 2 | the file itself is unacceptable (bad format, over `Limits`) | "this photo is too large to open" |
//! | 3 | the worker's stderr matched a memory-exhaustion signature | "this photo needs more memory than we can give it" |
//! | 4 | the worker panicked or was killed by a signal | "the image engine hit an unexpected error" |
//! | 5 | protocol violation (unreadable stdout, wrong length prefix) | "the image engine returned something unreadable" |
//! | 6 | the worker exceeded its wall-clock budget | "this photo took too long to open" |
//!
//! Code 1 is deliberately unused: it is what a shell reports for a generic
//! failure, and the worker never falls through to it, so an unexplained exit is
//! visible as *not* being any of our codes.
//!
//! # A sandbox is not permission to skip `Limits`
//!
//! [`crate::decode_bounded`] still enforces `Limits` in-process, and
//! `the_in_process_path_still_enforces_limits` asserts it. The sandbox is a
//! second line, not a replacement: it also costs a process spawn per image, which
//! is why the in-process path remains the default for batches of ordinary files.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::validate::Limits;

/// The hidden first argument that selects worker mode.
///
/// Deliberately obscure and version-tagged: a user who launches the binary with
/// this argument by accident should not get a decode worker, and a future
/// incompatible change to the job format should not be silently spoken for by an
/// old worker.
pub const WORKER_FLAG: &str = "--px-sandbox-worker-v1";

/// Exit codes. Documented in the module comment; kept as named constants so the
/// parent and the worker cannot disagree about a number.
pub mod exit {
    /// The file is unacceptable, but nothing went wrong.
    pub const FILE_REJECTED: i32 = 2;
    /// The worker hit the address-space ceiling.
    pub const MEMORY_EXHAUSTED: i32 = 3;
    /// The worker panicked or died on a signal.
    pub const ENGINE_FAILED: i32 = 4;
    /// The worker produced something the parent could not parse.
    pub const PROTOCOL_ERROR: i32 = 5;
    /// The worker outlived its wall-clock budget and was killed.
    pub const TIMED_OUT: i32 = 6;
}

/// How much memory the worker may address, in bytes.
///
/// The default is generous enough for a 40 MP RGBA buffer plus encoder scratch
/// (~160 MB decoded, plus working copies) and small enough that two concurrent
/// workers cannot exhaust a mid-range phone. The mobile profile is lower because
/// the process ceiling on Android is shared with the UI.
pub const DEFAULT_MEMORY_LIMIT: u64 = 512 * 1024 * 1024;
pub const DEFAULT_MOBILE_MEMORY_LIMIT: u64 = 256 * 1024 * 1024;

/// Wall-clock budget for one decode. Generous for a 40 MP file, far below the
/// point where a user would rather close the app.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// What the parent asked the worker to do.
///
/// Hand-rolled rather than JSON because the struct is three fields and this runs
/// per image: a serde round-trip on the spawn path would be pure overhead. The
/// encoding is length-prefixed binary so a hostile *job* cannot desynchronise the
/// stream the way a newline-delimited format could.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub input: Vec<u8>,
    pub max_memory_bytes: u64,
    /// Ceiling in bytes for the encoded result, or 0 for no ceiling.
    pub target_bytes: u64,
}

impl Job {
    fn write_to(&self, out: &mut impl Write) -> std::io::Result<()> {
        out.write_all(&(self.input.len() as u64).to_le_bytes())?;
        out.write_all(&self.input)?;
        out.write_all(&self.max_memory_bytes.to_le_bytes())?;
        out.write_all(&self.target_bytes.to_le_bytes())?;
        out.flush()
    }

    fn read_from(input: &mut impl Read) -> std::io::Result<Self> {
        let mut len_bytes = [0u8; 8];
        input.read_exact(&mut len_bytes)?;
        let len = u64::from_le_bytes(len_bytes);
        // The parent is trusted to send a sane length, but a corrupt stream must
        // not become a `usize::MAX` allocation. Cap at the point where a length
        // could not be legitimate anyway.
        if len > DEFAULT_MEMORY_LIMIT {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "job input length exceeds the address-space limit",
            ));
        }
        let mut bytes = vec![0u8; len as usize];
        input.read_exact(&mut bytes)?;
        let mut mem = [0u8; 8];
        input.read_exact(&mut mem)?;
        let mut target = [0u8; 8];
        input.read_exact(&mut target)?;
        Ok(Self {
            input: bytes,
            max_memory_bytes: u64::from_le_bytes(mem),
            target_bytes: u64::from_le_bytes(target),
        })
    }
}

/// Result of a sandboxed decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sandboxed {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub quality_used: u8,
}

/// Decode `input` in a child process under a hard memory ceiling.
///
/// Returns the same bytes as [`crate::decode_bounded`] would for a file within
/// limits, and a specific [`Error`] for each way it can fail.
pub fn decode_sandboxed(
    input: &[u8],
    limits: &Limits,
    memory_limit: u64,
    timeout: Duration,
) -> Result<Sandboxed> {
    let exe = worker_executable()?;
    let job = Job {
        input: input.to_vec(),
        max_memory_bytes: memory_limit,
        target_bytes: 0,
    };

    let spawn = Command::new(exe)
        .arg(WORKER_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // stderr is piped, never inherited: a panic backtrace is not a
        // user-facing error message and must not reach the parent's terminal.
        .stderr(Stdio::piped())
        .apply_memory_limit(memory_limit)
        .spawn()
        .map_err(|e| Error::Sandbox(format!("could not start the decode worker: {e}")))?;
    let mut child = spawn;

    // Write the job, then close stdin so the worker sees EOF and stops reading.
    // A failure here means the child died early; its exit status is the better
    // diagnostic, so report that instead.
    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| Error::Sandbox("worker stdin was not piped".into()))
        .and_then(|mut handle| {
            job.write_to(&mut handle)
                .map_err(|e| Error::Sandbox(format!("could not send the job: {e}")))
        });

    let started = Instant::now();
    let outcome = match write_result {
        Ok(()) => collect_with_timeout(&mut child, timeout, started)?,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Sandbox(format!(
                "could not send the job to the decode worker: {e}"
            )));
        }
    };

    classify(outcome, limits, memory_limit)
}

/// What came back from the child, before any interpretation.
struct RawOutcome {
    status: Option<std::process::ExitStatus>,
    stdout: Vec<u8>,
    stderr: String,
    timed_out: bool,
}

/// Read the child's pipes while enforcing the wall-clock budget.
///
/// `Child::wait_with_output` cannot time out, and blocking on `read_to_end`
/// before waiting means a worker that never exits hangs the UI forever. So the
/// pipes are drained on threads and the wait is polled, which is the only way to
/// get both "never block forever" and "never deadlock on a full pipe".
fn collect_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
    started: Instant,
) -> Result<RawOutcome> {
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stdout_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stderr_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    });

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    // Kill, then keep draining: the reader threads need the write
                    // end closed or they never see EOF.
                    let _ = child.kill();
                    timed_out = true;
                    break child.wait().ok();
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break None,
        }
    };

    Ok(RawOutcome {
        status,
        stdout: stdout_handle.join().unwrap_or_default(),
        stderr: stderr_handle.join().unwrap_or_default(),
        timed_out,
    })
}

/// Turn a raw outcome into a typed result.
///
/// The order of the checks is the order of specificity: a timeout is reported as
/// a timeout even though the kill also produced a signal, because "took too long"
/// and "ran out of memory" are different sentences and only one of them is true.
fn classify(outcome: RawOutcome, limits: &Limits, memory_limit: u64) -> Result<Sandboxed> {
    if outcome.timed_out {
        return Err(Error::Sandbox(format!(
            "this photo took too long to open: it was still running after {} seconds",
            DEFAULT_TIMEOUT.as_secs()
        )));
    }

    let Some(status) = outcome.status else {
        return Err(Error::Sandbox(
            "the decode worker could not be waited on".into(),
        ));
    };

    // A signal (SIGSEGV, SIGABRT, SIGKILL) has no exit code. On Unix, an
    // allocation failure under RLIMIT_AS surfaces as SIGABRT from Rust's
    // allocation error handler, which is why the stderr signature is checked
    // before falling back to a generic engine failure.
    #[cfg(unix)]
    let signalled = {
        use std::os::unix::process::ExitStatusExt;
        status.signal().is_some()
    };
    #[cfg(not(unix))]
    let signalled = false;

    let code = status.code();
    let memory_signature = looks_like_memory_exhaustion(&outcome.stderr);

    if memory_signature {
        return Err(Error::Sandbox(format!(
            "this photo needs more memory than the {memory_limit} byte ceiling \
             allows: it was refused rather than risking a crash"
        )));
    }

    match code {
        Some(0) => decode_response(&outcome.stdout),
        Some(exit::FILE_REJECTED) => Err(file_rejection(&outcome.stderr, limits)),
        Some(exit::ENGINE_FAILED) => Err(Error::Sandbox(
            "the image engine hit an unexpected error on this photo".into(),
        )),
        Some(exit::PROTOCOL_ERROR) => Err(Error::Sandbox(
            "the decode worker returned something unreadable".into(),
        )),
        Some(other) => Err(Error::Sandbox(format!(
            "the decode worker exited with an unexpected status {other}"
        ))),
        None if signalled => Err(Error::Sandbox(
            "the image engine crashed on this photo".into(),
        )),
        None => Err(Error::Sandbox(
            "the decode worker produced no exit status".into(),
        )),
    }
}

/// Turn the worker's stderr into a user-facing refusal.
///
/// The worker prints a machine-readable marker line, then prose. Only the prose
/// reaches the user, so a message can never leak an internal path or a Rust type
/// name.
fn file_rejection(stderr: &str, limits: &Limits) -> Error {
    let detail = stderr
        .lines()
        .find(|line| line.starts_with(REJECTION_MARKER))
        .map(|line| line.trim_start_matches(REJECTION_MARKER).trim())
        .unwrap_or("this photo could not be opened");
    Error::Sandbox(format!(
        "{detail} (limit: {} megapixels, {} px per side)",
        limits.max_pixels / 1_000_000,
        limits.max_dimension
    ))
}

const REJECTION_MARKER: &str = "px-reject: ";

/// Signatures of an allocation failure.
///
/// Matched on the child's stderr rather than trusted from it: a worker that
/// panicked for an unrelated reason prints something else, and a worker that was
/// killed for memory prints one of these. Rust's allocation error handler says
/// "memory allocation of N bytes failed", and glibc/musl abort paths mention
/// `std::alloc::handle_alloc_error` or `out of memory`.
fn looks_like_memory_exhaustion(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    [
        "memory allocation of",
        "out of memory",
        "cannot allocate memory",
        "handle_alloc_error",
        "failed to allocate",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Parse the worker's response: a length-prefixed buffer.
fn decode_response(stdout: &[u8]) -> Result<Sandboxed> {
    if stdout.len() < 8 {
        return Err(Error::Sandbox(
            "the decode worker returned a truncated response".into(),
        ));
    }
    let len = u64::from_le_bytes(stdout[0..8].try_into().expect("checked length"));
    let body = &stdout[8..];
    if body.len() != len as usize {
        return Err(Error::Sandbox(format!(
            "the decode worker promised {len} bytes and sent {}",
            body.len()
        )));
    }
    // width, height, quality
    if body.len() < 9 {
        return Err(Error::Sandbox(
            "the decode worker returned no dimensions".into(),
        ));
    }
    let (encoded, meta) = body.split_at(body.len() - 9);
    Ok(Sandboxed {
        width: u32::from_le_bytes(meta[0..4].try_into().expect("checked length")),
        height: u32::from_le_bytes(meta[4..8].try_into().expect("checked length")),
        quality_used: meta[8],
        bytes: encoded.to_vec(),
    })
}

/// Test-only shims.
///
/// The classification logic is the contract worth testing exhaustively, but it is
/// written against a [`RawOutcome`] the parent builds internally. These wrappers
/// expose it to `core/tests/sandbox.rs` without making the whole struct public,
/// which would invite a caller to hand the classifier an outcome it did not
/// observe.
#[doc(hidden)]
pub struct RawOutcomeForTest {
    pub status: Option<std::process::ExitStatus>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub timed_out: bool,
}

impl RawOutcomeForTest {
    /// Run the production classifier over a constructed outcome.
    pub fn classify_for_test(self, limits: &Limits, memory_limit: u64) -> Result<Sandboxed> {
        classify(
            RawOutcome {
                status: self.status,
                stdout: self.stdout,
                stderr: self.stderr,
                timed_out: self.timed_out,
            },
            limits,
            memory_limit,
        )
    }
}

/// Test-only: classify a bare exit status with no stdout or stderr.
///
/// Note the argument order: the observable facts first, the configuration last,
/// so a caller cannot pass a status without also saying what it was running under.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn classify_for_test(
    status: Option<std::process::ExitStatus>,
    stdout: Vec<u8>,
    stderr: String,
    timed_out: bool,
    limits: &Limits,
    memory_limit: u64,
) -> Result<Sandboxed> {
    classify(
        RawOutcome {
            status,
            stdout,
            stderr,
            timed_out,
        },
        limits,
        memory_limit,
    )
}

/// Test-only: parse a worker response without spawning anything.
#[doc(hidden)]
pub fn decode_response_for_test(stdout: &[u8]) -> Result<Sandboxed> {
    decode_response(stdout)
}

/// Locate the binary to re-exec.
///
/// `std::env::current_exe` is the test or application binary, which is what makes
/// this usable from a unit test at all: the same logic is exercised in-process.
/// An override exists so a caller that ships a dedicated helper can point at it.
fn worker_executable() -> Result<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("PIXELSMITH_SANDBOX_WORKER") {
        return Ok(std::path::PathBuf::from(path));
    }
    std::env::current_exe()
        .map_err(|e| Error::Sandbox(format!("cannot locate the decode worker binary: {e}")))
}

// ---------------------------------------------------------------------------
// Applying the limit from outside the child
// ---------------------------------------------------------------------------

/// Extension trait so the call site reads as one statement.
trait ApplyMemoryLimit {
    /// `&mut self` rather than `self`: `pre_exec` needs a mutable borrow, and
    /// consuming the builder here would break the fluent call site.
    fn apply_memory_limit(&mut self, bytes: u64) -> &mut Self;
}

impl ApplyMemoryLimit for Command {
    #[cfg(unix)]
    fn apply_memory_limit(&mut self, bytes: u64) -> &mut Self {
        use std::os::unix::process::CommandExt;
        // SAFETY: `pre_exec` runs in the child after fork and before exec, in a
        // process that has just come from fork and therefore may do very little:
        // no allocation, no threads, no locks. `setrlimit` is async-signal-safe
        // and touches no Rust state. `rlim_t` is u64 on every Unix Rust supports
        // (LP64 and LLP64 both make it 64-bit).
        unsafe {
            self.pre_exec(move || {
                let limit = libc::rlimit {
                    rlim_cur: bytes as libc::rlim_t,
                    // Hard limit equal to the soft limit: a soft limit is
                    // raisable up to the hard limit, so a worker that wants more
                    // memory must ask the kernel, and the kernel says no.
                    rlim_max: bytes as libc::rlim_t,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        self
    }

    /// Windows job objects are the equivalent, and are left for the phase that
    /// wires the desktop build: the runner has no Windows sandbox to test
    /// against, and an untested `unsafe` block calling `CreateJobObjectW` would be
    /// worse than an honest absence. The in-process `Limits` still bound the
    /// decode, which is the property that actually protects the user.
    #[cfg(windows)]
    fn apply_memory_limit(&mut self, _bytes: u64) -> &mut Self {
        self
    }

    #[cfg(not(any(unix, windows)))]
    fn apply_memory_limit(&mut self, _bytes: u64) -> &mut Self {
        self
    }
}

// ---------------------------------------------------------------------------
// Worker mode
// ---------------------------------------------------------------------------

/// Whether this process was started as a decode worker.
///
/// Called at the top of `main` by anything that wants the sandbox. A binary that
/// does not call it simply ignores the flag.
pub fn is_worker_invocation() -> bool {
    std::env::args().nth(1).as_deref() == Some(WORKER_FLAG)
}

/// Run one job and exit.
///
/// This is the code that runs where a hostile file can do damage, so it is
/// deliberately small: read a job, decode under `Limits`, write a length-prefixed
/// response, exit. Everything interesting happens in the parent.
pub fn run_worker() -> ! {
    let mut stdin = std::io::stdin();
    let job = match Job::read_from(&mut stdin) {
        Ok(job) => job,
        Err(e) => {
            eprintln!("{REJECTION_MARKER}the decode worker was sent an unreadable job: {e}");
            std::process::exit(exit::PROTOCOL_ERROR);
        }
    };

    // The worker reports its own ceiling so a refusal can name it. It cannot
    // *change* it: the parent set RLIMIT_AS in pre_exec, and the hard limit
    // equals the soft limit, so `setrlimit` to a higher value fails with EINVAL.
    let limits = Limits::mobile();

    let outcome = std::panic::catch_unwind(|| decode_job(&job, &limits));

    match outcome {
        Ok(Ok(response)) => {
            let mut out = std::io::stdout();
            let _ = out.write_all(&(response.bytes.len() as u64).to_le_bytes());
            let _ = out.write_all(&response.bytes);
            let _ = out.write_all(&response.width.to_le_bytes());
            let _ = out.write_all(&response.height.to_le_bytes());
            let _ = out.write_all(&response.quality_used.to_le_bytes());
            let _ = out.flush();
            std::process::exit(0);
        }
        Ok(Err(e)) => {
            // Only the message crosses back, and only after the marker, so the
            // parent can pick out the sentence meant for the user.
            eprintln!("{REJECTION_MARKER}{e}");
            std::process::exit(exit::FILE_REJECTED);
        }
        Err(_) => {
            // A panic message may name internal types; it stays on stderr, which
            // the parent captures and never shows.
            eprintln!("px-worker-panic");
            std::process::exit(exit::ENGINE_FAILED);
        }
    }
}

fn decode_job(job: &Job, limits: &Limits) -> Result<Sandboxed> {
    let img = crate::decode_bounded(&job.input, limits)?;
    let width = img.width();
    let height = img.height();
    let bytes = crate::format::encode(
        &img,
        crate::format::OutputFormat::Png,
        crate::format::EncodingOptions::default(),
    )?;
    Ok(Sandboxed {
        bytes,
        width,
        height,
        quality_used: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn a_job_round_trips_through_its_wire_format() {
        let job = Job {
            input: vec![1, 2, 3, 4, 5],
            max_memory_bytes: 1234,
            target_bytes: 5678,
        };
        let mut buf = Vec::new();
        job.write_to(&mut buf).expect("writing a job cannot fail");
        let back = Job::read_from(&mut Cursor::new(buf)).expect("reading a job back cannot fail");
        assert_eq!(job, back);
    }

    #[test]
    fn a_job_claiming_an_absurd_length_is_refused_rather_than_allocated() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&u64::MAX.to_le_bytes());
        let err = Job::read_from(&mut Cursor::new(buf)).expect_err("must refuse");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_truncated_response_is_a_protocol_error_not_a_panic() {
        for len in 0..8usize {
            let err = decode_response(&vec![0u8; len]).expect_err("must refuse");
            assert!(err.to_string().contains("truncated"), "{err}");
        }
        // Right length prefix, wrong body.
        let mut bad = Vec::new();
        bad.extend_from_slice(&99u64.to_le_bytes());
        bad.extend_from_slice(&[0u8; 10]);
        let err = decode_response(&bad).expect_err("must refuse");
        assert!(err.to_string().contains("promised 99"), "{err}");
    }

    #[test]
    fn a_well_formed_response_is_parsed() {
        let response = Sandboxed {
            bytes: vec![9, 9, 9],
            width: 12,
            height: 34,
            quality_used: 0,
        };
        let mut buf = Vec::new();
        buf.extend_from_slice(&(response.bytes.len() as u64).to_le_bytes());
        buf.extend_from_slice(&response.bytes);
        buf.extend_from_slice(&response.width.to_le_bytes());
        buf.extend_from_slice(&response.height.to_le_bytes());
        buf.extend_from_slice(&response.quality_used.to_le_bytes());

        let parsed = decode_response(&buf).expect("a well-formed response parses");
        assert_eq!(parsed.bytes, response.bytes);
        assert_eq!((parsed.width, parsed.height), (12, 34));
    }

    #[test]
    fn memory_exhaustion_is_recognised_from_the_usual_signatures() {
        for signature in [
            "memory allocation of 999999999 bytes failed",
            "std::alloc::handle_alloc_error",
            "Out of memory: Killed process",
            "cannot allocate memory for the request",
        ] {
            assert!(
                looks_like_memory_exhaustion(signature),
                "should have recognised {signature:?}"
            );
        }
        assert!(!looks_like_memory_exhaustion(
            "panicked at 'index out of bounds'"
        ));
        assert!(!looks_like_memory_exhaustion(""));
    }

    #[test]
    fn a_rejection_message_names_the_limit_and_leaks_nothing_internal() {
        let stderr =
            format!("{REJECTION_MARKER}image is larger than the input limit\nstack trace:");
        let message = file_rejection(&stderr, &Limits::mobile()).to_string();
        assert!(message.contains("input limit"), "{message}");
        assert!(message.contains("40 megapixels"), "{message}");
        assert!(!message.contains("stack trace"), "leaked stderr: {message}");
    }

    #[test]
    fn the_worker_flag_is_recognised_only_in_first_position() {
        // A flag in any other position is not a worker invocation: a user passing
        // it as a filename argument must not silently become a decode worker.
        assert!(is_worker_invocation_from(&[
            std::ffi::OsString::from("x"),
            std::ffi::OsString::from(WORKER_FLAG)
        ]));
        assert!(!is_worker_invocation_from(&[
            std::ffi::OsString::from(WORKER_FLAG),
            std::ffi::OsString::from("x")
        ]));
    }

    /// Split out so the argument shape can be tested without mutating the real
    /// process's argv.
    fn is_worker_invocation_from(args: &[std::ffi::OsString]) -> bool {
        args.get(1).map(|a| a == WORKER_FLAG).unwrap_or(false)
    }
}
