use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Folder the camera transfers into over Wi-Fi (e.g. FUJIFILM PC AutoSave).
    pub inbox: Option<PathBuf>,
    /// Library folder new shots are copied to, e.g. `\\SHIELD\Samsung_T7\Photos`.
    pub import_to: Option<PathBuf>,
    /// Number of files copied to the library at once.
    pub parallel_copies: usize,
    /// Mirror ratings and colour labels into `.xmp` sidecars next to RAWs.
    pub write_xmp: bool,
    /// Explicit editor path; Photoshop is auto-detected when unset.
    pub editor_path: Option<PathBuf>,
    /// Open the RAF (via Camera Raw) instead of the JPEG for RAW+JPEG pairs.
    pub edit_raw: bool,
    /// On-device "search by what's in the photo" (downloads a model once).
    pub smart_search: bool,
    /// Analyse photos on the graphics card when there is one (DirectML).
    pub smart_gpu: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            inbox: None,
            import_to: None,
            parallel_copies: 4,
            write_xmp: true,
            editor_path: None,
            edit_raw: true,
            smart_search: false,
            smart_gpu: true,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
    }
}

/// Finds the newest installed Photoshop.
pub fn find_photoshop() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let roots = [
            std::env::var_os("ProgramFiles"),
            Some("C:\\Program Files".into()),
        ];
        for root in roots.into_iter().flatten() {
            let adobe = PathBuf::from(root).join("Adobe");
            let mut installs: Vec<PathBuf> = std::fs::read_dir(&adobe)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("Adobe Photoshop")
                })
                .map(|e| e.path().join("Photoshop.exe"))
                .filter(|p| p.is_file())
                .collect();
            // "Adobe Photoshop 2026" sorts after "Adobe Photoshop 2025".
            installs.sort();
            if let Some(newest) = installs.pop() {
                return Some(newest);
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        let mut installs: Vec<PathBuf> = std::fs::read_dir("/Applications")
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("Adobe Photoshop")
            })
            .map(|e| e.path())
            .collect();
        installs.sort();
        installs.pop()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}
