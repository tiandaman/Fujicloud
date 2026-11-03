//! What this PC can do, for choosing how big a model each analysis uses.

use serde::{Deserialize, Serialize};

/// The processors, memory and graphics card found.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hardware {
    /// Logical processors.
    pub processors: usize,
    /// Installed memory, in GB.
    pub memory_gb: f32,
    /// The graphics card DirectML would run on (a discrete one first).
    pub gpu: Option<String>,
    /// Its own memory, in GB (0 for one that shares the PC's).
    pub gpu_memory_gb: f32,
}

/// How big the models are: light ones for a PC with little to spare, the
/// standard ones, or bigger, slower and more accurate ones for a PC with
/// power to spare.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Light,
    #[default]
    Standard,
    Best,
}

/// The choice in Settings: let the PC decide, or say.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    #[default]
    Auto,
    Light,
    Standard,
    Best,
}

impl Hardware {
    pub fn detect() -> Self {
        let (gpu, gpu_memory_gb) = gpu();
        Hardware {
            processors: std::thread::available_parallelism().map_or(1, |n| n.get()),
            memory_gb: memory_bytes() as f32 / (1u64 << 30) as f32,
            gpu,
            gpu_memory_gb,
        }
    }

    /// The size that suits this PC. The bigger models need about 3 GB more
    /// memory and run several times as long, which a graphics card with
    /// its own memory or a dozen processors makes up for. The light ones
    /// keep smart search's first run as short as before on 4 processors or
    /// less than 8 GB (SigLIP 2's picture model does three times the work).
    pub fn suits(&self) -> Level {
        let fast = self.gpu_memory_gb >= 4.0 || self.processors >= 12;
        if fast && self.memory_gb >= 15.0 {
            Level::Best
        } else if self.processors <= 4 || self.memory_gb < 7.5 {
            Level::Light
        } else {
            Level::Standard
        }
    }

    pub fn level(&self, choice: Choice) -> Level {
        match choice {
            Choice::Light => Level::Light,
            Choice::Standard => Level::Standard,
            Choice::Best => Level::Best,
            Choice::Auto => self.suits(),
        }
    }
}

#[cfg(feature = "smart-search")]
fn gpu() -> (Option<String>, f32) {
    crate::embed::best_gpu().map_or((None, 0.0), |g| (Some(g.name), g.vram_mb as f32 / 1024.0))
}

#[cfg(not(feature = "smart-search"))]
fn gpu() -> (Option<String>, f32) {
    (None, 0.0)
}

#[cfg(windows)]
fn memory_bytes() -> u64 {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `status` is a properly sized MEMORYSTATUSEX.
    match unsafe { GlobalMemoryStatusEx(&mut status) } {
        Ok(()) => status.ullTotalPhys,
        Err(_) => 0,
    }
}

#[cfg(not(windows))]
fn memory_bytes() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|m| {
            m.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map_or(0, |kb| kb * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bigger_models_for_a_pc_with_power_to_spare() {
        let pc = |processors, memory_gb, gpu_memory_gb| Hardware {
            processors,
            memory_gb,
            gpu: None,
            gpu_memory_gb,
        };
        // A 4-core laptop with 8 GB, and an 8-thread one with 6 GB.
        assert_eq!(pc(4, 8.0, 0.0).level(Choice::Auto), Level::Light);
        assert_eq!(pc(8, 5.9, 0.0).level(Choice::Auto), Level::Light);
        // An 8-thread laptop with 16 GB and graphics built into the processor.
        assert_eq!(pc(8, 15.8, 0.5).level(Choice::Auto), Level::Standard);
        // A gaming laptop: an 8 GB graphics card and 16 GB.
        assert_eq!(pc(8, 15.8, 8.0).level(Choice::Auto), Level::Best);
        // A 16-thread desktop with 32 GB and no graphics card.
        assert_eq!(pc(16, 32.0, 0.0).level(Choice::Auto), Level::Best);
        // Plenty of processors but too little memory for the bigger models.
        assert_eq!(pc(16, 8.0, 0.0).level(Choice::Auto), Level::Standard);
        // The choice in Settings wins either way.
        assert_eq!(pc(4, 8.0, 0.0).level(Choice::Best), Level::Best);
        assert_eq!(pc(16, 32.0, 8.0).level(Choice::Light), Level::Light);
        assert_eq!(pc(16, 32.0, 8.0).level(Choice::Standard), Level::Standard);
        let here = Hardware::detect();
        assert!(here.processors >= 1 && here.memory_gb > 0.5, "{here:?}");
    }
}
