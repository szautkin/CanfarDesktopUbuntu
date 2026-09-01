//! One lock around cfitsio, because cfitsio has one error stack.
//!
//! The bindings are raw `fitsio-sys` FFI, and cfitsio keeps its error messages
//! in a process-global stack: `ffgmsg` pops from the same place whichever
//! thread is asking. Two decodes in flight at once therefore read each other's
//! errors — and worse than reading them, they free them.
//!
//! That is not a hypothetical. Opening four cubes in a row, three of them
//! failures, reported the missing file's message for the malformed one and the
//! malformed one's message for the missing file, and then:
//!
//! ```text
//! double free or corruption (out)
//! ```
//!
//! The cube loader decodes on a worker thread while the FITS viewer, the shape
//! sniffer and the native-slice reader all call cfitsio from wherever they
//! happen to be, so the overlap needs no unusual timing.
//!
//! The remedy for a non-reentrant C library is the obvious one: let one caller
//! in at a time. The lock is held for a whole open-read-close sequence, not per
//! FFI call — a `fitsfile*` and the error stack behind it are only coherent for
//! the length of the operation that owns them.
//!
//! The cost is that two cubes decode one after the other instead of together.
//! They already did: the work is I/O and cfitsio, and cfitsio was never running
//! two of them safely.

use std::sync::{Mutex, MutexGuard};

static CFITSIO: Mutex<()> = Mutex::new(());

/// Wait for exclusive use of cfitsio.
///
/// Poisoning is ignored: the lock guards no data of ours, only the C library's
/// own state, and a thread that panicked mid-decode has left cfitsio no worse
/// than a thread that returned an error. Refusing every later read because one
/// panicked would turn a bad file into a broken application.
pub fn acquire() -> MutexGuard<'static, ()> {
    let guard = CFITSIO.lock().unwrap_or_else(|e| e.into_inner());
    // Start from an empty error stack.
    //
    // A cfitsio failure queues SEVERAL messages and `ffgmsg` pops one, so the
    // rest stay for whoever asks next. Opening a text file left cards of it on
    // the stack, and the following open — of a path that did not exist —
    // reported them: "Cannot open FITS file: [package] name = \"verbinal\"".
    //
    // Cleared on acquisition rather than after each read: what matters is that
    // a message this operation reads belongs to this operation, and that is a
    // property of where it STARTS.
    #[cfg(feature = "fits")]
    unsafe {
        fitsio_sys::ffcmsg();
    }
    guard
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lock is re-entrant across sequential callers, and exclusive.
    ///
    /// A guard dropped at the end of one call must leave the next able to take
    /// it; that is the whole contract, and it is worth pinning because a lock
    /// that is never released looks exactly like a slow decode.
    #[test]
    fn one_caller_at_a_time_and_the_next_gets_in() {
        {
            let _first = acquire();
        }
        let _second = acquire();
    }

    /// Every safe wrapper over the raw FFI takes the lock.
    ///
    /// The failure this catches is a new reader added beside the existing ones
    /// that calls cfitsio without it — which does not fail, it corrupts, and
    /// only sometimes.
    #[test]
    fn every_cfitsio_caller_goes_through_the_lock() {
        let mut unguarded: Vec<String> = Vec::new();
        for (path, source) in crate::testing::rust_sources() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "cfitsio_lock.rs" {
                continue;
            }
            let code = crate::testing::code(&source);
            if !code.contains("fitsio_sys as sys") {
                continue;
            }
            // A safe fn that steps into `unsafe { … }` is an entry point; the
            // `unsafe fn`s below it are already inside one.
            for (i, line) in code.lines().enumerate() {
                let t = line.trim();
                if !t.starts_with("unsafe {") && !t.contains("= unsafe {") {
                    continue;
                }
                let lines: Vec<&str> = code.lines().take(i).collect();
                let before = lines[lines.len().saturating_sub(6)..].join("\n");
                // A site may opt out only by saying why, at the site. The
                // two that do are `Drop` impls for a handle created and
                // dropped inside one locked section: the caller holds the lock
                // already, and `Mutex` is not reentrant.
                let exempt = before.contains("No lock here:");
                if !before.contains("cfitsio_lock::acquire") && !exempt {
                    unguarded.push(format!("{name}:{}: {t}", i + 1));
                }
            }
        }
        assert!(
            unguarded.is_empty(),
            "these enter cfitsio without the lock, so they can race another \
             decode over its process-global error stack: {unguarded:#?}"
        );
    }

    /// Every `ffgmsg` buffer is the size cfitsio writes.
    ///
    /// `ffgmsg` writes up to `FLEN_ERRMSG` (81) bytes into whatever it is
    /// handed. Both call sites gave it a 31-byte STACK array, so a cfitsio
    /// message longer than thirty characters wrote up to fifty bytes past the
    /// end of it — "failed to find or open the following file: (ffopen)" is
    /// one, and opening a path that did not exist ended in `double free or
    /// corruption (out)` and took the application with it.
    ///
    /// A literal here is the whole bug, so the literal is what is forbidden.
    #[test]
    fn an_error_buffer_is_the_size_cfitsio_writes() {
        let mut wrong: Vec<String> = Vec::new();
        for (path, source) in crate::testing::rust_sources() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "cfitsio_lock.rs" {
                continue;
            }
            let code = crate::testing::code(&source);
            for (i, line) in code.lines().enumerate() {
                let t = line.trim();
                if !t.contains("ffgmsg(") {
                    continue;
                }
                let lines: Vec<&str> = code.lines().take(i).collect();
                let before = lines[lines.len().saturating_sub(3)..].join("\n");
                if !before.contains("FLEN_ERRMSG") {
                    wrong.push(format!("{name}:{}: {t}", i + 1));
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "these hand `ffgmsg` a buffer that is not FLEN_ERRMSG, which it \
             will write past: {wrong:#?}"
        );
    }

    /// A panic while holding it does not lock everyone out.
    #[test]
    fn a_panicked_holder_does_not_poison_the_library() {
        let panicked = std::thread::spawn(|| {
            let _g = acquire();
            panic!("a decode gave up");
        })
        .join();
        assert!(panicked.is_err(), "the test thread should have panicked");
        // Still usable.
        let _g = acquire();
    }
}
