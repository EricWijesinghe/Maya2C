//! Write-ahead trial journal, keyed by GPU UUID (ADR-137 HW-1, S-4).
//!
//! Before a step is shown to the user, the Hub writes the entry
//! `{uuid, driver, default, previous, trial}` as `trying` and a separate
//! `trying` marker, and syncs both. On start, a marker without a matching
//! finished entry, a `trying` entry, an unreadable journal, a journal with
//! unknown fields, or one another user could have written all count as "a
//! trial was in progress": fail closed. Recovery tells the user to go back
//! to the driver default read live from the driver, never a value from here.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const JOURNAL_FILE: &str = "gpu-tuning-journal.json";
pub const MARKER_FILE: &str = "gpu-tuning.trying";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    Trying,
    Done,
    Bad,
}

/// The last step of one GPU.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub driver: String,
    pub default_w: f64,
    pub previous_w: f64,
    pub trial_w: f64,
    pub state: EntryState,
}

/// A step that ended a trial badly, for one driver version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BadStep {
    pub driver: String,
    pub trial_w: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    #[serde(default)]
    pub entries: BTreeMap<String, Entry>,
    #[serde(default)]
    pub bad: BTreeMap<String, Vec<BadStep>>,
}

/// UUID to the trial watts of every step shown and not yet finished.
pub type Marker = BTreeMap<String, f64>;

impl Journal {
    /// The highest step marked bad for this card and driver: no step at or
    /// below it is suggested again.
    #[must_use]
    pub fn bad_at_or_below(&self, uuid: &str, driver: &str) -> Option<f64> {
        self.bad
            .get(uuid)?
            .iter()
            .filter(|b| b.driver == driver && b.trial_w.is_finite())
            .map(|b| b.trial_w)
            .reduce(f64::max)
    }
}

/// The UUIDs whose last shown step never finished, from the files on disk.
#[must_use]
pub fn in_progress(journal: &Journal, marker: &Marker) -> BTreeSet<String> {
    let open = journal
        .entries
        .iter()
        .filter(|(_, e)| e.state == EntryState::Trying)
        .map(|(uuid, _)| uuid.clone());
    let unmatched = marker
        .iter()
        .filter(|(uuid, trial)| {
            journal
                .entries
                .get(*uuid)
                .is_none_or(|e| e.state == EntryState::Trying || e.trial_w != **trial)
        })
        .map(|(uuid, _)| uuid.clone());
    open.chain(unmatched).collect()
}

/// True when an account other than the current user could have written
/// `path`. Such a file is never trusted.
fn foreign(path: &Path) -> Result<bool, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(foreign_unix(
            meta.uid(),
            meta.mode(),
            rustix::process::getuid().as_raw(),
        ))
    }
    #[cfg(windows)]
    {
        crate::key_acl::others_can_access(path)
    }
}

/// Owned by someone else, or writable by group or others.
#[cfg(unix)]
#[must_use]
pub const fn foreign_unix(file_uid: u32, mode: u32, me: u32) -> bool {
    const GROUP_OR_OTHER_WRITE: u32 = 0o022;
    file_uid != me || mode & GROUP_OR_OTHER_WRITE != 0
}

/// Reads a JSON file this user owns. `Ok(None)` for a missing file; an
/// unreadable, malformed or foreign file is an error the caller fails closed on.
pub fn load_owned<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if foreign(path)? {
        return Err(format!(
            "{}: another account can write this file, so it is not trusted",
            path.display()
        ));
    }
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes through a synced temp file and a rename, so a crash keeps either
/// the old file or the new one, never half of one.
pub fn write_synced(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let dir = path.parent().ok_or("a tuning file has no folder")?;
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    // A leftover temp file keeps its old mode; start from none.
    remove(&tmp)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Owner-only, so `load_owned` never reads its own file as foreign under a loose umask.
        options.mode(0o600);
    }
    let mut file = options
        .open(&tmp)
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    file.write_all(&text)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(())
}

/// Removes a file; a file already gone is fine.
fn remove(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("{}: {e}", path.display()))
        }
        _ => Ok(()),
    }
}

/// The journal and marker in the Hub's data folder.
pub struct Store {
    journal: PathBuf,
    marker: PathBuf,
}

impl Store {
    #[must_use]
    pub fn new(dir: &Path) -> Self {
        Self {
            journal: dir.join(JOURNAL_FILE),
            marker: dir.join(MARKER_FILE),
        }
    }

    /// Both files. Any error means "do not trust either".
    pub fn load(&self) -> Result<(Journal, Marker), String> {
        let journal = load_owned(&self.journal)?.unwrap_or_default();
        let marker = load_owned(&self.marker)?.unwrap_or_default();
        Ok((journal, marker))
    }

    /// Records a step as `trying` in both files, journal first, before the
    /// step is shown.
    pub fn begin(
        &self,
        journal: &mut Journal,
        marker: &mut Marker,
        uuid: &str,
        entry: Entry,
    ) -> Result<(), String> {
        marker.insert(uuid.to_string(), entry.trial_w);
        journal.entries.insert(uuid.to_string(), entry);
        write_synced(&self.journal, journal)?;
        write_synced(&self.marker, marker)
    }

    /// Closes a step: the journal says how it ended, then the marker drops
    /// it. A crash between the two leaves a marker that matches a finished
    /// entry, which [`in_progress`] reads as finished.
    pub fn end(
        &self,
        journal: &mut Journal,
        marker: &mut Marker,
        uuid: &str,
        state: EntryState,
    ) -> Result<(), String> {
        if let Some(e) = journal.entries.get_mut(uuid) {
            if state == EntryState::Bad {
                journal
                    .bad
                    .entry(uuid.to_string())
                    .or_default()
                    .push(BadStep {
                        driver: e.driver.clone(),
                        trial_w: e.trial_w,
                    });
            }
            e.state = state;
        }
        write_synced(&self.journal, journal)?;
        marker.remove(uuid);
        if marker.is_empty() {
            remove(&self.marker)
        } else {
            write_synced(&self.marker, marker)
        }
    }

    /// Starts both files over, when the user turns tuning on after the old
    /// ones could not be trusted.
    pub fn reset(&self) -> Result<(), String> {
        write_synced(&self.journal, &Journal::default())?;
        remove(&self.marker)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hub-journal-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    pub fn entry(trial_w: f64) -> Entry {
        Entry {
            driver: "581.29".into(),
            default_w: 250.0,
            previous_w: 250.0,
            trial_w,
            state: EntryState::Trying,
        }
    }

    #[test]
    fn a_begun_step_is_in_progress_until_it_ends() {
        let dir = scratch("begin");
        let store = Store::new(&dir);
        let (mut j, mut m) = store.load().expect("no files is a clean start");
        assert!(in_progress(&j, &m).is_empty());
        store
            .begin(&mut j, &mut m, "GPU-a", entry(235.0))
            .expect("begin");
        let (j2, m2) = store.load().expect("reload");
        assert_eq!(in_progress(&j2, &m2), BTreeSet::from(["GPU-a".to_string()]));
        store
            .end(&mut j, &mut m, "GPU-a", EntryState::Done)
            .expect("end");
        let (j3, m3) = store.load().expect("reload");
        assert!(in_progress(&j3, &m3).is_empty());
        assert!(!dir.join(MARKER_FILE).exists(), "no open step, no marker");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_marker_without_a_finished_entry_or_an_edited_entry_is_in_progress() {
        let mut m = Marker::new();
        m.insert("GPU-a".into(), 235.0);
        assert!(
            in_progress(&Journal::default(), &m).contains("GPU-a"),
            "journal deleted"
        );

        let mut j = Journal::default();
        j.entries.insert(
            "GPU-a".into(),
            Entry {
                state: EntryState::Done,
                ..entry(220.0)
            },
        );
        assert!(
            in_progress(&j, &m).contains("GPU-a"),
            "a finished entry for another step"
        );

        j.entries.insert(
            "GPU-a".into(),
            Entry {
                state: EntryState::Done,
                ..entry(235.0)
            },
        );
        assert!(
            in_progress(&j, &m).is_empty(),
            "crash between closing journal and marker"
        );

        j.entries.insert("GPU-b".into(), entry(200.0));
        assert!(
            in_progress(&j, &Marker::new()).contains("GPU-b"),
            "marker deleted"
        );
    }

    #[test]
    fn a_journal_with_unknown_fields_or_garbage_is_refused() {
        let dir = scratch("garbage");
        fs::create_dir_all(&dir).expect("dir");
        let store = Store::new(&dir);
        fs::write(dir.join(JOURNAL_FILE), "{ not json").expect("seed");
        assert!(store.load().is_err());
        fs::write(
            dir.join(JOURNAL_FILE),
            r#"{"entries":{},"bad":{},"done":true}"#,
        )
        .expect("seed");
        assert!(store.load().is_err(), "an unknown field is not ignored");
        fs::write(dir.join(JOURNAL_FILE), "{}").expect("seed");
        fs::write(dir.join(MARKER_FILE), "[1]").expect("seed");
        assert!(store.load().is_err(), "an unreadable marker is refused too");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_step_is_kept_per_card_and_driver() {
        let dir = scratch("bad");
        let store = Store::new(&dir);
        let (mut j, mut m) = (Journal::default(), Marker::new());
        store
            .begin(&mut j, &mut m, "GPU-a", entry(205.0))
            .expect("begin");
        store
            .end(&mut j, &mut m, "GPU-a", EntryState::Bad)
            .expect("end");
        assert_eq!(j.bad_at_or_below("GPU-a", "581.29"), Some(205.0));
        assert_eq!(
            j.bad_at_or_below("GPU-a", "590.00"),
            None,
            "another driver starts clean"
        );
        assert_eq!(
            j.bad_at_or_below("GPU-b", "581.29"),
            None,
            "another card starts clean"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_another_account_can_write_is_foreign() {
        assert!(!foreign_unix(1000, 0o100_600, 1000));
        assert!(foreign_unix(1001, 0o100_600, 1000), "another owner");
        assert!(foreign_unix(1000, 0o100_666, 1000), "world-writable");
    }
}
