//! Append-only diagnostic log for the notebook kernel's lifecycle.
//!
//! Port of `Services/Notebook/NotebookLogger.cs`. Deliberately not a logging
//! framework — the app has no logger, and pulling one in for a handful of lines
//! would be a large dependency for a small need.
//!
//! It exists because kernel failures are the hardest thing in this app to
//! diagnose after the fact: the interesting output (a Python traceback, an
//! interpreter that could not start) appears once in a dialog and is then gone.
//! A dated file the user can attach to a bug report is the difference between
//! "the notebook broke" and an actionable report.
//!
//! Every operation is best-effort and silent on failure. A logger that panicked,
//! or that surfaced its own I/O errors, would be worse than no logger at all.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// How many days of logs to keep. Old files are removed on first use.
const RETENTION_DAYS: i64 = 7;

/// Serialises writes from the GTK thread and the tokio pool, both of which log.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// `~/.local/share/verbinal/logs` (or the platform equivalent).
pub fn log_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("net", "canfar", "Verbinal").map(|d| d.data_dir().join("logs"))
}

/// Today's log file.
fn log_path() -> Option<PathBuf> {
    let dir = log_dir()?;
    let day = chrono::Local::now().format("%Y-%m-%d");
    Some(dir.join(format!("notebook-{day}.log")))
}

pub fn info(message: &str) {
    write("INFO", message);
}

pub fn warn(message: &str) {
    write("WARN", message);
}

pub fn error(message: &str) {
    write("ERROR", message);
}

fn write(level: &str, message: &str) {
    let Some(path) = log_path() else { return };
    let Some(dir) = path.parent() else { return };

    let _guard = WRITE_LOCK.lock();
    // A poisoned lock is not a reason to stop logging — the previous holder
    // panicking is exactly when the log matters most, so the guard is taken for
    // ordering only and its error deliberately ignored.
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    prune_old_logs(dir);

    let line = format!(
        "{} [{level}] {message}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Delete logs older than [`RETENTION_DAYS`].
///
/// Only files matching our own `notebook-YYYY-MM-DD.log` naming are considered,
/// so this can never remove something another tool put in the directory.
fn prune_old_logs(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let cutoff = chrono::Local::now().date_naive() - chrono::Duration::days(RETENTION_DAYS);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(day) = parse_log_day(name) else {
            continue;
        };
        if day < cutoff {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The date encoded in a `notebook-YYYY-MM-DD.log` filename, or `None` when the
/// name is not one of ours.
fn parse_log_day(file_name: &str) -> Option<chrono::NaiveDate> {
    let rest = file_name.strip_prefix("notebook-")?;
    let day = rest.strip_suffix(".log")?;
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_own_log_names_are_recognised() {
        assert!(parse_log_day("notebook-2026-08-11.log").is_some());
        // Anything else in the directory must be left alone — pruning is a
        // delete, so a loose match here would destroy someone else's file.
        assert!(parse_log_day("notebook-2026-08-11.log.bak").is_none());
        assert!(parse_log_day("kernel-2026-08-11.log").is_none());
        assert!(parse_log_day("notebook-not-a-date.log").is_none());
        assert!(parse_log_day("README.md").is_none());
        assert!(parse_log_day("notebook-.log").is_none());
    }

    #[test]
    fn pruning_keeps_recent_days_and_removes_old_ones() {
        let dir = std::env::temp_dir().join(format!("verbinal_log_prune_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let today = chrono::Local::now().date_naive();
        let recent = dir.join(format!("notebook-{today}.log"));
        let old = dir.join(format!(
            "notebook-{}.log",
            today - chrono::Duration::days(RETENTION_DAYS + 1)
        ));
        let foreign = dir.join("someone-elses.log");
        for p in [&recent, &old, &foreign] {
            std::fs::write(p, b"x").unwrap();
        }

        prune_old_logs(&dir);

        assert!(recent.exists(), "today's log must survive");
        assert!(!old.exists(), "a log past the retention window is removed");
        assert!(foreign.exists(), "an unrelated file must never be touched");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
