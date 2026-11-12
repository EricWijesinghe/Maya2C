//! Per-GPU tuning results, keyed by the driver's device UUID (ADR-137, HW-1).
//! One JSON file in the Hub's data folder. A profile is a hint: on load it is
//! clamped to the live driver range, and one for another driver version is
//! discarded. The Hub never applies it; it shows it.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::autotune::Range;
use crate::journal::{load_owned, write_synced};

pub const PROFILE_FILE: &str = "gpu-tuning.json";

/// The limit one guided search kept on one card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub driver: String,
    pub best_w: f64,
    pub floor_pct: f64,
    pub gain_pct: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileFile {
    #[serde(default)]
    pub gpus: BTreeMap<String, Profile>,
}

impl ProfileFile {
    /// The saved limit for this card, inside today's driver range, or `None`
    /// when there is none, it is not a number, or the driver changed.
    #[must_use]
    pub fn hint(&self, uuid: &str, driver: &str, range: Range) -> Option<f64> {
        let p = self.gpus.get(uuid)?;
        (p.driver == driver && p.best_w.is_finite())
            .then(|| p.best_w.clamp(range.min_w, range.default_w))
    }
}

/// Reads the file. Missing is empty; unreadable, unknown or foreign is an
/// error, and the caller then neither shows nor overwrites it.
pub fn load(path: &Path) -> Result<ProfileFile, String> {
    load_owned(path).map(Option::unwrap_or_default)
}

pub fn save(path: &Path, file: &ProfileFile) -> Result<(), String> {
    write_synced(path, file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_gpu::SIM_RANGE;

    fn profile(best_w: f64) -> Profile {
        Profile {
            driver: "581.29".into(),
            best_w,
            floor_pct: 95.0,
            gain_pct: 9.4,
        }
    }

    #[test]
    fn a_profile_round_trips_through_the_file_and_stays_keyed_by_uuid() {
        let dir = std::env::temp_dir().join(format!("hub-profiles-{}", std::process::id()));
        let path = dir.join(PROFILE_FILE);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            load(&path),
            Ok(ProfileFile::default()),
            "no file is no profiles"
        );
        let mut file = ProfileFile::default();
        file.gpus.insert("GPU-a".into(), profile(220.0));
        file.gpus.insert("GPU-b".into(), profile(190.0));
        save(&path, &file).expect("save creates the folder");
        let back = load(&path).expect("reload");
        assert_eq!(back, file);
        assert_eq!(back.hint("GPU-a", "581.29", SIM_RANGE), Some(220.0));
        assert_eq!(
            back.hint("GPU-b", "581.29", SIM_RANGE),
            Some(190.0),
            "one card never overwrites another"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stored_profile_outside_the_driver_range_is_clamped_or_discarded() {
        let mut file = ProfileFile::default();
        file.gpus.insert("low".into(), profile(1.0));
        file.gpus.insert("high".into(), profile(10_000.0));
        file.gpus.insert("nan".into(), profile(f64::NAN));
        assert_eq!(file.hint("low", "581.29", SIM_RANGE), Some(SIM_RANGE.min_w));
        assert_eq!(
            file.hint("high", "581.29", SIM_RANGE),
            Some(SIM_RANGE.default_w)
        );
        assert_eq!(file.hint("nan", "581.29", SIM_RANGE), None);
        assert_eq!(
            file.hint("low", "590.00", SIM_RANGE),
            None,
            "another driver: discarded"
        );
        assert_eq!(file.hint("GPU-z", "581.29", SIM_RANGE), None);
    }

    #[test]
    fn a_corrupt_or_extended_file_is_an_error_not_an_empty_set() {
        let dir = std::env::temp_dir().join(format!("hub-profiles-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(PROFILE_FILE);
        std::fs::write(&path, "{ not json").expect("seed");
        assert!(
            load(&path).is_err(),
            "refusing to load is what keeps the file safe"
        );
        std::fs::write(&path, r#"{"gpus":{},"active":{}}"#).expect("seed");
        assert!(load(&path).is_err(), "an unknown field is refused");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
