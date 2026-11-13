//! A small log file, so a problem on someone's PC leaves a trace they can
//! send: `%LOCALAPPDATA%\\app.fujicloud.desktop\\logs\\fujicloud.log` on Windows.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

use parking_lot::Mutex;

struct FileLog(Mutex<File>);

impl log::Log for FileLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        // Our own messages from info up; other crates only when it matters.
        let ours = m.target().starts_with("fujicloud") || m.target().starts_with("fuji_core");
        m.level()
            <= if ours {
                log::Level::Info
            } else {
                log::Level::Warn
            }
    }

    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let _ = writeln!(
                self.0.lock(),
                "{}",
                line(&now.to_string(), r.level(), &r.args().to_string())
            );
        }
    }

    fn flush(&self) {
        let _ = self.0.lock().flush();
    }
}

/// One line of the log: when, a word only when something's wrong, then
/// what happened. More lines of the same message are set in under it.
fn line(now: &str, level: log::Level, text: &str) -> String {
    let word = match level {
        log::Level::Error => "PROBLEM",
        log::Level::Warn => "WARNING",
        _ => "",
    };
    let text = text
        .trim_end()
        .replace('\n', "\n                               ");
    format!("{now}  {word:<7}  {text}")
}

/// A warning or problem from the log.
#[derive(Debug, PartialEq, serde::Serialize)]
pub struct Entry {
    /// `2026-11-13 17:40:09`
    pub at: String,
    pub problem: bool,
    pub text: String,
}

/// Picks the warnings and problems out of the log's text, oldest first.
/// Reads this format and the one before it (`WARN`, `ERROR`).
fn problems(text: &str) -> Vec<Entry> {
    let mut found: Vec<Entry> = Vec::new();
    let mut open = false;
    for l in text.lines() {
        let stamped = l.len() > 20
            && l.is_char_boundary(19)
            && l.as_bytes()[4] == b'-'
            && l.as_bytes()[13] == b':'
            && l.as_bytes()[..4].iter().all(u8::is_ascii_digit);
        if !stamped {
            // More of the message above.
            if open
                && !l.trim().is_empty()
                && !l.starts_with('=')
                && let Some(last) = found.last_mut()
            {
                last.text.push('\n');
                last.text.push_str(l.trim());
            }
            continue;
        }
        let rest = l[19..].trim_start();
        let level = [
            ("PROBLEM", true),
            ("ERROR", true),
            ("WARNING", false),
            ("WARN", false),
        ]
        .into_iter()
        .find(|(word, _)| rest.starts_with(&format!("{word} ")));
        open = level.is_some();
        if let Some((word, problem)) = level {
            found.push(Entry {
                at: l[..19].to_owned(),
                problem,
                text: rest[word.len()..].trim().to_owned(),
            });
        }
    }
    found
}

/// The last `most` warnings and problems in the log, newest first.
pub fn recent(dir: &Path, most: usize) -> Vec<Entry> {
    use std::io::{Read, Seek, SeekFrom};
    let mut text = Vec::new();
    if let Ok(mut f) = File::open(dir.join("fujicloud.log")) {
        // The end of it is plenty.
        let len = f.metadata().map_or(0, |m| m.len());
        let _ = f.seek(SeekFrom::Start(len.saturating_sub(512 << 10)));
        let _ = f.read_to_end(&mut text);
    }
    let mut found = problems(&String::from_utf8_lossy(&text));
    found.reverse();
    found.truncate(most);
    found
}

pub fn init(dir: &Path, version: &str) {
    let _ = fs::create_dir_all(dir);
    let path = dir.join("fujicloud.log");
    // Keep the previous file when this one gets big.
    if fs::metadata(&path).is_ok_and(|m| m.len() > 4 << 20) {
        let _ = fs::rename(&path, dir.join("fujicloud.old.log"));
    }
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    #[cfg(windows)]
    last_words::install(&file);
    // Each run of the app under its own heading.
    let started = chrono::Local::now().format("%A %-d %B %Y, %H:%M");
    let _ = writeln!(
        &file,
        "\n======== Fujicloud {version} opened {started} ========"
    );
    if log::set_boxed_logger(Box::new(FileLog(Mutex::new(file)))).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        log::error!("problem in {}: {info}", thread.name().unwrap_or("a worker"));
        // On Windows stderr is this log too (see `last_words`): once is enough.
        if !cfg!(windows) {
            default(info);
        }
    }));
    log::info!("Fujicloud {version} started");
}

/// How the program ended, when it ended badly. Rust's own last words (a
/// stack overflow, running out of memory) go to stderr, which an app without
/// a console doesn't have, and a crash in code that isn't Rust (ONNX Runtime,
/// a graphics driver, a Windows codec) says nothing at all. Both go to the
/// log instead, so a log sent after a crash says what happened and where.
#[cfg(windows)]
mod last_words {
    use std::fs::File;
    use std::io::Write;
    use std::os::windows::io::IntoRawHandle;
    use std::sync::OnceLock;

    use windows_sys::Win32::System::Console::{STD_ERROR_HANDLE, SetStdHandle};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        EXCEPTION_POINTERS, SetUnhandledExceptionFilter,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    };

    static LOG: OnceLock<File> = OnceLock::new();

    pub fn install(log: &File) {
        let (Ok(stderr), Ok(crashes)) = (log.try_clone(), log.try_clone()) else {
            return;
        };
        // SAFETY: Windows gets a handle of ours that is never closed, and a
        // filter that only reads what it is handed.
        unsafe {
            SetStdHandle(STD_ERROR_HANDLE, stderr.into_raw_handle());
            let _ = LOG.set(crashes);
            SetUnhandledExceptionFilter(Some(crashed));
        }
    }

    /// Notes the exception and the DLL it happened in, then lets Windows
    /// end the program as it would have.
    unsafe extern "system" fn crashed(info: *const EXCEPTION_POINTERS) -> i32 {
        // SAFETY: Windows hands over a valid record for the duration.
        let record = unsafe { &*(*info).ExceptionRecord };
        let at = record.ExceptionAddress as usize;
        let mut module = std::ptr::null_mut();
        let mut name = [0u16; 260];
        // SAFETY: looks up which loaded module holds the address.
        let len = unsafe {
            if GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                at as *const u16,
                &mut module,
            ) != 0
            {
                GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) as usize
            } else {
                0
            }
        };
        if let Some(mut log) = LOG.get() {
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let _ = write!(
                log,
                "{now} ERROR crashed: exception {:#010x} in ",
                record.ExceptionCode as u32
            );
            for c in char::decode_utf16(name[..len.min(name.len())].iter().copied()) {
                let _ = write!(log, "{}", c.unwrap_or('?'));
            }
            let _ = writeln!(
                log,
                " at +{:#x}, in {}",
                at.wrapping_sub(module as usize),
                std::thread::current().name().unwrap_or("a worker")
            );
        }
        0 // EXCEPTION_CONTINUE_SEARCH
    }
}

#[cfg(all(test, windows))]
mod tests {
    /// Started as its own process by `a_crash_is_logged`: crashes for real.
    #[test]
    #[ignore]
    fn crash_now() {
        let Some(log) = std::env::var_os("FUJICLOUD_CRASH_LOG") else {
            return;
        };
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .unwrap();
        super::last_words::install(&file);
        eprintln!("last words before the crash");
        // SAFETY: none: reading address 8 is the crash under test. No
        // "stopped working" box, so CI doesn't wait on it.
        unsafe {
            use windows_sys::Win32::System::Diagnostics::Debug::{
                SEM_NOGPFAULTERRORBOX, SetErrorMode,
            };
            SetErrorMode(SEM_NOGPFAULTERRORBOX);
            std::ptr::read_volatile(8 as *const u8);
        }
    }

    #[test]
    fn a_crash_is_logged() {
        let log = std::env::temp_dir().join(format!("fujicloud-crash-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&log);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "logfile::tests::crash_now",
                "--nocapture",
            ])
            .env("FUJICLOUD_CRASH_LOG", &log)
            .status()
            .unwrap();
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let _ = std::fs::remove_file(&log);
        assert!(!status.success(), "it was meant to crash");
        assert!(text.contains("last words before the crash"), "{text}");
        assert!(text.contains("crashed: exception 0xc0000005 in "), "{text}");
        assert!(text.to_lowercase().contains(".exe at +0x"), "{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problems_are_picked_out_of_the_log() {
        let text = [
            "======== Fujicloud 0.2.0 opened Friday 13 November 2026, 17:40 ========".to_owned(),
            line(
                "2026-11-13 17:40:01",
                log::Level::Info,
                "library open: 2 folders",
            ),
            line(
                "2026-11-13 17:40:09",
                log::Level::Warn,
                "pets: couldn't read C:\\Photos\\IMG_1.HEIC: not a picture\nsecond line",
            ),
            "2026-11-12 09:00:00 ERROR problem in a worker: boom".to_owned(),
            line("2026-11-13 17:41:00", log::Level::Info, "done"),
        ]
        .join("\n");
        let found = problems(&text);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].at, "2026-11-13 17:40:09");
        assert!(!found[0].problem);
        assert_eq!(
            found[0].text,
            "pets: couldn't read C:\\Photos\\IMG_1.HEIC: not a picture\nsecond line"
        );
        assert!(found[1].problem);
        assert_eq!(found[1].text, "problem in a worker: boom");
    }
}
