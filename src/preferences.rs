//! User defaults, accessed only during instance creation and explicit editor actions.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

fn preference_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    base.map(|base| base.join("PortalSurfer/GainSnap/default-mode"))
}

fn load_at(path: &Path) -> bool {
    !matches!(
        std::fs::read_to_string(path).as_deref().map(str::trim),
        Ok("peak")
    )
}

/// Load the last explicit Peak/RMS selection, falling back to RMS.
pub(crate) fn default_rms_mode() -> bool {
    preference_path().is_none_or(|path| load_at(&path))
}

fn save_at(path: &Path, rms: bool) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing preference directory"))?;
    std::fs::create_dir_all(parent)?;
    let serial = NEXT_WRITE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".default-mode-{}-{serial}", std::process::id()));
    let result = (|| {
        std::fs::write(&temporary, if rms { "rms\n" } else { "peak\n" })?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Remember an explicit editor selection without involving realtime processing.
pub(crate) fn remember_rms_mode(rms: bool) {
    if let Some(path) = preference_path() {
        if let Err(error) = save_at(&path, rms) {
            eprintln!("GainSnap could not save the default Peak/RMS mode: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_survives_fresh_reads_and_the_last_choice_wins() {
        let dir = std::env::temp_dir().join(format!("gainsnap-preference-{}", std::process::id()));
        let path = dir.join("nested/default-mode");
        assert!(load_at(&path));
        save_at(&path, false).unwrap();
        assert!(!load_at(&path));
        save_at(&path, true).unwrap();
        assert!(load_at(&path));
        save_at(&path, false).unwrap();
        assert!(!load_at(&path));
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_or_missing_defaults_fall_back_and_write_errors_are_reported() {
        let dir = std::env::temp_dir().join(format!(
            "gainsnap-invalid-preference-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("default-mode");
        std::fs::write(&path, "corrupt").unwrap();
        assert!(load_at(&path));
        assert!(save_at(&path.join("child"), false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "corrupt");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
