//! Per-GPU tuning profiles, keyed by the driver's device UUID (ADR-137, HW-1).
//! One JSON file in the Hub's data folder. `active` records every GPU left
//! tuned, so a Hub that died mid-tune resets those GPUs on its next start.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::gpu_control::Knobs;

/// The best setting one GPU reached, and the stock it returns to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub stock: Knobs,
    pub best: Knobs,
    pub gain_pct: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileFile {
    /// Device UUID to its saved profile.
    #[serde(default)]
    pub gpus: BTreeMap<String, Profile>,
    /// Device UUID to the stock settings to restore, for every GPU left tuned.
    #[serde(default)]
    pub active: BTreeMap<String, Knobs>,
}

/// Reads the file. A missing file is an empty set. Anything else unreadable
/// is an error, so the caller refuses to write over it.
pub fn load(path: &Path) -> Result<ProfileFile, String> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ProfileFile::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Writes through a temp file and a rename, so a crash mid-write keeps the old file.
pub fn save(path: &Path, file: &ProfileFile) -> Result<(), String> {
    let text = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOCK: Knobs = Knobs {
        power_w: 250.0,
        clock_mhz: Some(2700.0),
    };
    const TUNED: Knobs = Knobs {
        power_w: 150.0,
        clock_mhz: Some(1890.0),
    };

    fn sample_file() -> ProfileFile {
        let mut f = ProfileFile::default();
        f.gpus.insert(
            "GPU-a".into(),
            Profile {
                stock: STOCK,
                best: TUNED,
                gain_pct: 12.5,
            },
        );
        f.gpus.insert(
            "GPU-b".into(),
            Profile {
                stock: STOCK,
                best: STOCK,
                gain_pct: 0.0,
            },
        );
        f.active.insert("GPU-b".into(), STOCK);
        f
    }

    #[test]
    fn a_profile_round_trips_through_json_and_stays_keyed_by_uuid() {
        let file = sample_file();
        let text = serde_json::to_string(&file).expect("serialize");
        let back: ProfileFile = serde_json::from_str(&text).expect("parse");
        assert_eq!(back, file);
        assert_eq!(
            back.gpus["GPU-a"].best, TUNED,
            "a second card never overwrites the first"
        );
        assert_eq!(back.gpus["GPU-b"].best, STOCK);
    }

    #[test]
    fn a_saved_file_loads_back_and_a_missing_one_is_empty() {
        let dir = std::env::temp_dir().join(format!("hub-profiles-{}", std::process::id()));
        let path = dir.join("gpu-tuning.json");
        assert_eq!(
            load(&path),
            Ok(ProfileFile::default()),
            "no file is no profiles"
        );
        save(&path, &sample_file()).expect("save creates the folder");
        assert_eq!(load(&path), Ok(sample_file()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_an_empty_set() {
        let dir = std::env::temp_dir().join(format!("hub-profiles-bad-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("gpu-tuning.json");
        fs::write(&path, "{ not json").expect("seed");
        assert!(
            load(&path).is_err(),
            "refusing to load is what keeps the file safe"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
