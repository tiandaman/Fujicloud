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
            let _ = writeln!(self.0.lock(), "{now} {:<5} {}", r.level(), r.args());
        }
    }

    fn flush(&self) {
        let _ = self.0.lock().flush();
    }
}

pub fn init(dir: &Path) {
    let _ = fs::create_dir_all(dir);
    let path = dir.join("fujicloud.log");
    // Keep the previous file when this one gets big.
    if fs::metadata(&path).is_ok_and(|m| m.len() > 4 << 20) {
        let _ = fs::rename(&path, dir.join("fujicloud.old.log"));
    }
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    if log::set_boxed_logger(Box::new(FileLog(Mutex::new(file)))).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        log::error!("problem in {}: {info}", thread.name().unwrap_or("a worker"));
        default(info);
    }));
    log::info!("Fujicloud {} started", env!("CARGO_PKG_VERSION"));
}
