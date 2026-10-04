//! The engine's process entry point.
//!
//! Two jobs, chosen by the first argument:
//!
//! * `--px-sandbox-worker-v1` reads a decode job from stdin and writes the encoded
//!   result to stdout. This is the process [`pixelsmith_core::sandbox`] spawns
//!   under an address-space limit it sets itself.
//! * no argument, the ABI dump, which is what `verify.sh` and
//!   `scripts/check-dart-bindings.sh` call.
//!
//! It exists as a binary rather than as a mode inside the Flutter app because the
//! sandbox needs a process that shares no address space with the UI, and on
//! Android and Windows the app's own executable is the only one we can re-exec.

use pixelsmith_core::sandbox;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Worker mode first, before anything else touches stdin. `run_worker` never
    // returns; it exits with a code from the documented contract.
    if sandbox::is_worker_invocation() {
        // Test-only probe flags, handled here rather than in the library so the
        // shipped worker surface stays small. An unrecognised flag is not an
        // error: it must not turn into a hang.
        match args.get(1).map(String::as_str) {
            Some("--print-rlimit-as") => {
                print_current_limit();
                std::process::exit(0);
            }
            Some("--try-raise-rlimit-as") => {
                let requested: u64 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(u64::MAX);
                std::process::exit(try_raise_limit(requested));
            }
            Some("--panic") => {
                // Reached only by the sandbox test. The panic must be caught by
                // the worker's `catch_unwind`, not by the process, so the parent's
                // classification is exercised rather than a raw abort.
                let outcome = std::panic::catch_unwind(|| {
                    panic!("deliberate panic from the sandbox test");
                });
                eprintln!("px-worker-panic");
                std::process::exit(match outcome {
                    Ok(()) => sandbox::exit::ENGINE_FAILED,
                    Err(_) => sandbox::exit::ENGINE_FAILED,
                });
            }
            Some("--sleep-forever") => {
                // Bounded, so a test that provokes it cannot hang the suite. The
                // point is that the *parent* enforces the timeout, not the child.
                std::thread::sleep(std::time::Duration::from_secs(30));
                std::process::exit(sandbox::exit::TIMED_OUT);
            }
            _ => {}
        }
        sandbox::run_worker();
    }

    abi_dump(&args)
}

/// Print the address-space ceiling currently in force.
///
/// This is how the test proves the parent - not the worker - chose the limit: the
/// child reports what the kernel says, and the parent set both rlim_cur and
/// rlim_max to the same value before `exec`.
#[cfg(unix)]
fn print_current_limit() {
    // SAFETY: `getrlimit` only writes into the provided struct. RLIMIT_AS is a
    // valid resource on every Unix Rust supports.
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let rc = unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut limit) };
    if rc != 0 {
        eprintln!("getrlimit failed: {}", std::io::Error::last_os_error());
        std::process::exit(sandbox::exit::PROTOCOL_ERROR);
    }
    println!("{}", limit.rlim_cur);
}

/// Ask for a bigger address space, and report whether the kernel allowed it.
///
/// The parent sets `rlim_max` equal to `rlim_cur`, so raising the soft limit above
/// the hard limit must fail with `EINVAL`. A non-zero exit is the expected result;
/// zero would mean a compromised worker could opt out of its own containment.
#[cfg(unix)]
fn try_raise_limit(requested: u64) -> i32 {
    let wanted = libc::rlimit {
        rlim_cur: requested as libc::rlim_t,
        rlim_max: requested as libc::rlim_t,
    };
    // SAFETY: `setrlimit` reads the provided struct. The soft limit cannot be
    // raised above the hard limit, so this cannot grant more than is permitted.
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_AS, &wanted) };
    if rc == 0 {
        // It should not have succeeded. Report it as a protocol error so the test
        // notices rather than reading a comfortable exit code as agreement.
        eprintln!("the worker raised its own address-space limit");
        sandbox::exit::PROTOCOL_ERROR
    } else {
        // Confirm the limit is unchanged after the failed attempt.
        let mut after = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let _ = unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut after) };
        println!("refused; still {}", after.rlim_cur);
        sandbox::exit::FILE_REJECTED
    }
}

#[cfg(not(unix))]
fn print_current_limit() {
    // No address-space rlimit on this platform. The in-process `Limits` are the
    // containment; see the module comment in `sandbox.rs`.
    eprintln!("no RLIMIT_AS on this platform");
    std::process::exit(sandbox::exit::PROTOCOL_ERROR);
}

#[cfg(not(unix))]
fn try_raise_limit(_requested: u64) -> i32 {
    sandbox::exit::PROTOCOL_ERROR
}

fn abi_dump(args: &[String]) -> ExitCode {
    use pixelsmith_core::ffi_abi::{
        ENTRY_POINTS, dart_binding_declarations, dart_drift, px_buffer_layout,
    };

    match args.first().map(String::as_str) {
        Some("--check") => {
            let Some(path) = args.get(1) else {
                eprintln!("--check needs the path to bindings.dart");
                return ExitCode::from(2);
            };
            let Ok(source) = std::fs::read_to_string(path) else {
                eprintln!("cannot read {path}");
                return ExitCode::from(2);
            };
            let drift = dart_drift(&source);
            if drift.is_empty() {
                println!(
                    "{path}: {} entry points match the engine ABI",
                    ENTRY_POINTS.len()
                );
                return ExitCode::SUCCESS;
            }
            eprintln!(
                "{path}: {} declaration(s) do not match the engine:",
                drift.len()
            );
            for line in &drift {
                eprintln!("  {line}");
            }
            ExitCode::FAILURE
        }
        Some(other) => {
            eprintln!("unknown flag: {other}");
            ExitCode::from(2)
        }
        None => {
            println!("// Generated by `cargo run --bin px-abi-dump`. Do not hand-edit.");
            println!(
                "// Engine {}: {} entry points.",
                pixelsmith_core::ENGINE_VERSION,
                ENTRY_POINTS.len()
            );
            println!("// Reference output. `cargo run --bin px-abi-dump -- --check` is the gate.");
            println!();
            print!("{}", dart_binding_declarations());
            let layout = px_buffer_layout();
            println!(
                "// PxBuffer: {} bytes, {} byte alignment",
                layout.size, layout.align
            );
            for field in &layout.fields {
                println!(
                    "//   {:<6} offset {:>2}, size {}",
                    field.name, field.offset, field.size
                );
            }
            ExitCode::SUCCESS
        }
    }
}
