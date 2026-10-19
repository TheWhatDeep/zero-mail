//! Logging (section 13, item 8; SPEC_GAPS G51, G57): `tracing` to rotating files in
//! `<app data>/logs`, five files of 2 MB (`zeromail.log` and `zeromail.log.1` to `.4`), at
//! level info. The debug switch (setting `debug_logging`, off by default) adds the IMAP and
//! SMTP traces, which redact credentials and message data themselves. Nothing in this app
//! logs message content, subjects or credentials at any level.
//!
//! `tracing-appender` rotates by time, so the size-based rotation is this file's writer.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{reload, EnvFilter, Registry};

/// The setting that turns the protocol traces on (SPEC_GAPS G57).
pub const DEBUG_SETTING: &str = "debug_logging";
/// Size of one log file before it rotates.
pub const FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Log files kept, the current one included.
pub const FILES: usize = 5;
const FILE_NAME: &str = "zeromail.log";

/// The log folder under the app data directory.
pub fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

/// The filter: info everywhere, plus the protocol traces when debugging.
fn filter(debug: bool) -> EnvFilter {
    let mut f = EnvFilter::new("info");
    if debug {
        for target in [mail_imap::TRACE_TARGET, mail_smtp::TRACE_TARGET] {
            if let Ok(d) = format!("{target}=debug").parse() {
                f = f.add_directive(d);
            }
        }
    }
    f
}

static RELOAD: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();

/// Starts logging to `<data_dir>/logs`. Called once at startup; a second call, or a
/// subscriber set elsewhere (tests), leaves the existing one in place.
pub fn init(data_dir: &Path) -> io::Result<()> {
    let writer = RotatingFiles::open(log_dir(data_dir), FILE_BYTES, FILES)?;
    let (layer, handle) = reload::Layer::new(filter(false));
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true);
    if tracing_subscriber::registry()
        .with(layer)
        .with(fmt)
        .try_init()
        .is_ok()
    {
        let _ = RELOAD.set(handle);
    }
    Ok(())
}

/// Turns the IMAP and SMTP traces on or off. Connections opened afterwards follow it.
pub fn set_debug(debug: bool) {
    if let Some(handle) = RELOAD.get() {
        let _ = handle.reload(filter(debug));
    }
}

/// Size-rotating log files: when the current file would pass `max_bytes`, `.3` becomes
/// `.4` (the old `.4` is dropped), and so on down to the current file becoming `.1`.
#[derive(Debug, Clone)]
pub struct RotatingFiles {
    inner: Arc<Mutex<Files>>,
}

#[derive(Debug)]
struct Files {
    dir: PathBuf,
    max_bytes: u64,
    keep: usize,
    file: File,
    written: u64,
}

impl RotatingFiles {
    pub fn open(dir: PathBuf, max_bytes: u64, keep: usize) -> io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(FILE_NAME);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            inner: Arc::new(Mutex::new(Files {
                dir,
                max_bytes,
                keep: keep.max(1),
                file,
                written,
            })),
        })
    }
}

impl Files {
    fn rotate(&mut self) -> io::Result<()> {
        let name = |n: usize| {
            if n == 0 {
                self.dir.join(FILE_NAME)
            } else {
                self.dir.join(format!("{FILE_NAME}.{n}"))
            }
        };
        let oldest = name(self.keep - 1);
        if oldest.exists() {
            std::fs::remove_file(&oldest)?;
        }
        for n in (0..self.keep - 1).rev() {
            let from = name(n);
            if from.exists() {
                std::fs::rename(&from, name(n + 1))?;
            }
        }
        self.file = OpenOptions::new().create(true).append(true).open(name(0))?;
        self.written = 0;
        Ok(())
    }

    fn write_line(&mut self, buf: &[u8]) -> io::Result<()> {
        if self.written > 0 && self.written + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(buf)?;
        self.written += buf.len() as u64;
        Ok(())
    }
}

/// One event's output, written as a whole when the event is done.
#[derive(Debug)]
pub struct LogLine {
    files: Arc<Mutex<Files>>,
    buf: Vec<u8>,
}

impl Write for LogLine {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LogLine {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            let mut files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
            // A log write that fails is dropped; logging never stops the app.
            let _ = files.write_line(&self.buf);
        }
    }
}

impl<'a> MakeWriter<'a> for RotatingFiles {
    type Writer = LogLine;

    fn make_writer(&'a self) -> Self::Writer {
        LogLine {
            files: Arc::clone(&self.inner),
            buf: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_rotate_by_size_and_keep_five() {
        let dir = std::env::temp_dir().join(format!("zm-logs-{}", mail_render::nonce()));
        let files = RotatingFiles::open(dir.clone(), 100, 5).unwrap();
        for i in 0..40 {
            let mut w = files.make_writer();
            writeln!(w, "line {i:03} {}", "x".repeat(30)).unwrap();
        }
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "zeromail.log",
                "zeromail.log.1",
                "zeromail.log.2",
                "zeromail.log.3",
                "zeromail.log.4"
            ]
        );
        for name in &names {
            let len = std::fs::metadata(dir.join(name)).unwrap().len();
            assert!(len <= 100, "{name}: {len}");
        }
        let current = std::fs::read_to_string(dir.join("zeromail.log")).unwrap();
        assert!(current.contains("line 039"));
        let oldest = std::fs::read_to_string(dir.join("zeromail.log.4")).unwrap();
        assert!(
            !oldest.contains("line 000"),
            "the oldest lines were dropped"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn debug_adds_only_the_protocol_traces() {
        let off = filter(false).to_string();
        assert!(!off.contains("trace"), "{off}");
        let on = filter(true).to_string();
        assert!(
            on.contains("mail_imap::trace=debug") && on.contains("mail_smtp::trace=debug"),
            "{on}"
        );
        assert!(on.contains("info"));
    }
}
