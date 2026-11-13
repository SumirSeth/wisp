//! Best-effort panic evidence, including in panic=abort builds.
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const LIMIT: u64 = 256 * 1024;
const KEEP: u64 = 64 * 1024;

fn append(path: &Path, entry: &str) -> std::io::Result<()> {
    if fs::metadata(path).is_ok_and(|m| m.len() > LIMIT) {
        let mut file = fs::File::open(path)?;
        file.seek(SeekFrom::End(-(KEEP as i64)))?;
        let mut tail = Vec::new();
        file.read_to_end(&mut tail)?;
        // Start with a whole line (also avoids splitting a UTF-8 character).
        let start = tail.iter().position(|b| *b == b'\n').map_or(0, |i| i + 1);
        fs::write(path, &tail[start..])?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(entry.as_bytes())
}

pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        let thread = std::thread::current();
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-string panic");
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let channel = if crate::channel::NIGHTLY { "nightly" } else { "stable" };
        // Escape newlines so each panic is one searchable log record.
        let entry = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} Wisp {} {} thread={} panic={} at={} backtrace={}\n",
            time.wYear,
            time.wMonth,
            time.wDay,
            time.wHour,
            time.wMinute,
            time.wSecond,
            time.wMilliseconds,
            env!("CARGO_PKG_VERSION"),
            channel,
            thread.name().unwrap_or("unnamed").replace(['\r', '\n'], " "),
            message.replace(['\r', '\n'], " "),
            location.replace(['\r', '\n'], " "),
            std::backtrace::Backtrace::force_capture()
                .to_string()
                .replace(['\r', '\n'], " ")
        );
        let _ = append(&crate::util::data_dir().join("wisp.log"), &entry);
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotates_large_log_and_preserves_recent_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wisp.log");
        fs::write(&path, "old line\n".repeat(40000) + "recent\n").unwrap();
        append(&path, "panic record\n").unwrap();
        let log = fs::read_to_string(&path).unwrap();
        assert!(log.len() <= KEEP as usize + 13);
        assert!(log.ends_with("recent\npanic record\n"));
    }
    #[test]
    fn small_log_appends_and_io_errors_are_returned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wisp.log");
        append(&path, "one\n").unwrap();
        append(&path, "two\n").unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "one\ntwo\n");
        assert!(append(dir.path(), "ignored").is_err());
    }
}
