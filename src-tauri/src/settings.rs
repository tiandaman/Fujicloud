use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A folder new shots arrive in: FUJIFILM PC AutoSave, iCloud Photos, …
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Inbox {
    pub path: PathBuf,
    /// Move files to the Recycle Bin once their copy in the library has been
    /// checked. Never on for iCloud: deleting there deletes from the phone.
    #[serde(default)]
    pub cleanup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Folders the camera and phone send new shots to.
    pub inboxes: Vec<Inbox>,
    /// The single inbox of earlier versions, read once to migrate.
    #[serde(skip_serializing)]
    pub inbox: Option<PathBuf>,
    /// A second drive or synced folder the library is mirrored to.
    pub backup_to: Option<PathBuf>,
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
    /// Find faces and group them into people (downloads two small models).
    pub faces: bool,
    /// Find cats and dogs and tell them apart (downloads two small models).
    pub pets: bool,
    /// Bumped by "Rebuild thumbnails" so the window stops reusing the old
    /// images it has cached.
    pub cache_version: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            inboxes: Vec::new(),
            inbox: None,
            backup_to: None,
            import_to: None,
            parallel_copies: 4,
            write_xmp: true,
            editor_path: None,
            edit_raw: true,
            smart_search: false,
            smart_gpu: true,
            faces: false,
            pets: false,
            cache_version: 0,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        let mut settings: Self = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        settings.migrate();
        settings
    }

    /// Settings saved by earlier versions, or sent by the UI, in today's form.
    pub fn migrate(&mut self) {
        if let Some(path) = self.inbox.take()
            && !self.inboxes.iter().any(|i| i.path == path)
        {
            self.inboxes.insert(
                0,
                Inbox {
                    path,
                    cleanup: false,
                },
            );
        }
        for inbox in &mut self.inboxes {
            inbox.cleanup &= !is_cloud_folder(&inbox.path);
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
    }
}

/// Folders synced with a cloud service, where deleting a file deletes it
/// everywhere (iCloud Photos removes it from the iPhone too).
pub fn is_cloud_folder(path: &Path) -> bool {
    let p = path.to_string_lossy().to_lowercase();
    [
        "icloud",
        "onedrive",
        "dropbox",
        "google drive",
        "googledrive",
    ]
    .iter()
    .any(|c| p.contains(c))
}

/// Inbox folders that exist on this PC: PC AutoSave's and iCloud Photos'.
pub fn suggested_inboxes() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) else {
        return Vec::new();
    };
    [
        home.join("Pictures").join("FUJIFILM PC AutoSave"),
        home.join("Pictures").join("iCloud Photos").join("Photos"),
        home.join("iCloudPhotos").join("Photos"),
    ]
    .into_iter()
    .filter(|p| p.is_dir())
    .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_old_single_inbox_carries_over() {
        let old = br#"{"inbox": "C:\\Users\\me\\Pictures\\FUJIFILM PC AutoSave", "importTo": "\\\\SHIELD\\T7"}"#;
        let dir = std::env::temp_dir().join(format!("fujicloud-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, old).unwrap();

        let settings = Settings::load(&path);
        assert_eq!(
            settings.inboxes,
            [Inbox {
                path: r"C:\Users\me\Pictures\FUJIFILM PC AutoSave".into(),
                cleanup: false
            }]
        );
        settings.save(&path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains("\"inbox\""), "{saved}");
        assert_eq!(Settings::load(&path).inboxes.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn synced_folders_are_never_cleaned_up() {
        let mut settings = Settings {
            inboxes: vec![
                Inbox {
                    path: r"C:\Users\me\Pictures\iCloud Photos\Photos".into(),
                    cleanup: true,
                },
                Inbox {
                    path: r"C:\Users\me\Pictures\FUJIFILM PC AutoSave".into(),
                    cleanup: true,
                },
            ],
            ..Settings::default()
        };
        settings.migrate();
        assert!(!settings.inboxes[0].cleanup);
        assert!(settings.inboxes[1].cleanup);
        assert!(is_cloud_folder(Path::new(r"C:\Users\me\OneDrive\Pictures")));
        assert!(!is_cloud_folder(Path::new(r"\\SHIELD\Samsung_T7\Photos")));
    }
}
