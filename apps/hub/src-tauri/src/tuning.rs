//! Hub GPU tuner, the driver (ADR-137 HW-1, stage T1a: guided). One session
//! per GPU UUID the user turned Tune on for. It feeds the decision core
//! (`autotune.rs`) the latest readings, records every step in the
//! write-ahead journal before the step is shown, and keeps the profile file.
//!
//! It writes nothing to any card and needs no rights: the user sets each
//! limit in their own tool and the Hub reads it back. An automatic stage
//! (T1b, an elevated helper) is not in this build; ADR-137 gates it on a
//! release signing key and a per-machine install, neither of which exists.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::Serialize;

use crate::autotune::{Phase, Range, Reason, Sample, Step, Tuner, efficiency, rollback_guidance};
use crate::gpu::GpuStat;
use crate::journal::{Entry, EntryState, Journal, Marker, Store, in_progress};
use crate::miner::{MinerState, device_key};
use crate::profiles::{self, PROFILE_FILE, Profile, ProfileFile};

/// One GPU's guided search.
struct Session {
    tuner: Tuner,
    driver: String,
    /// The latest efficiency read-out, hash rate per watt.
    hps_per_w: Option<f64>,
}

/// Every GPU's tuning state, behind one mutex shared with the supervisor.
pub struct Tuning {
    store: Store,
    journal: Journal,
    marker: Marker,
    /// The journal could not be trusted: tuning is off for every GPU until
    /// the user turns it on again, which starts the journal over.
    untrusted: Option<String>,
    profile_path: PathBuf,
    profiles: ProfileFile,
    profiles_unreadable: Option<String>,
    /// UUIDs whose last step never finished, with the words shown for them.
    unfinished: BTreeMap<String, String>,
    sessions: BTreeMap<String, Session>,
    notice: Option<String>,
}

/// One GPU's line on its rig card.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuneView {
    pub uuid: String,
    pub on: bool,
    /// What to do now, in plain words.
    pub line: String,
    /// The power limit to set now, when there is one. The UI shows the
    /// command for it; the Hub never runs it.
    pub set_w: Option<f64>,
    pub hps_per_w: Option<f64>,
    pub floor_pct: Option<f64>,
    /// The limit an earlier search kept, clamped to today's range.
    pub saved_w: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    pub devices: Vec<TuneView>,
    pub notice: Option<String>,
}

/// Locks the tuning state, recovering from a poisoned mutex.
pub fn lock(state: &Mutex<Tuning>) -> MutexGuard<'_, Tuning> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The words and the limit to set for one phase.
fn instruction(phase: Phase, range: Range, ceiling_c: f64) -> (String, Option<f64>) {
    let d = range.default_w.round();
    match phase {
        Phase::WaitDefault => (
            format!(
                "Step 1: start mining on this card with its power limit at the default, {d} W."
            ),
            Some(d),
        ),
        Phase::Baseline => (
            format!("Measuring at the default {d} W. Leave it running."),
            None,
        ),
        Phase::Await(w) => (
            format!(
                "Next step: set the power limit to {w} W. The Hub measures once it reads {w} W back."
            ),
            Some(w),
        ),
        Phase::Measure(w) => (format!("Measuring at {w} W. Leave it running."), None),
        Phase::Done(w) if w < range.default_w => (
            format!("Done: keep the power limit at {w} W. It holds your hash-rate floor."),
            Some(w),
        ),
        Phase::Done(_) => (
            format!("Done: the default {d} W was already the best. Nothing to change."),
            None,
        ),
        // On heat no command is offered: the default is a higher limit, and
        // the words say to wait until the card has cooled.
        Phase::Aborted(Reason::OverTemperature) => (
            rollback_guidance(Reason::OverTemperature, range, ceiling_c),
            None,
        ),
        Phase::Aborted(reason) => (rollback_guidance(reason, range, ceiling_c), Some(d)),
    }
}

/// This GPU's reading. A field the driver or the miner did not give stays `None`.
fn sample_for(dev: Option<&GpuStat>, workers: &[MinerState]) -> Sample {
    let Some(dev) = dev else {
        return Sample::default();
    };
    let key = device_key(Some(dev.index));
    let worker = workers.iter().find(|w| w.device == key);
    Sample {
        hps: worker.and_then(|w| w.rates.back()).map(|r| r.hps),
        power_w: dev.power_w,
        limit_w: dev.power_limit_w,
        temp_c: dev.temp_c,
        running: worker.is_some_and(|w| w.running),
        restarts: worker.map_or(0, |w| w.restarts),
        rejected: worker.map_or(0, |w| w.rejected),
        hw_errors: worker.map_or(0, |w| w.hw_errors),
    }
}

impl Tuning {
    /// Opens the journal and profiles. A step left unfinished by an earlier
    /// run is recorded as bad and shown; tuning starts off for every GPU.
    #[must_use]
    pub fn open(dir: &Path) -> Self {
        let profile_path = dir.join(PROFILE_FILE);
        let (profiles, profiles_unreadable) = match profiles::load(&profile_path) {
            Ok(file) => (file, None),
            Err(why) => (ProfileFile::default(), Some(why)),
        };
        let store = Store::new(dir);
        let (journal, marker, untrusted) = match store.load() {
            Ok((j, m)) => (j, m, None),
            Err(why) => (Journal::default(), Marker::new(), Some(why)),
        };
        let mut tuning = Self {
            store,
            journal,
            marker,
            untrusted,
            profile_path,
            profiles,
            profiles_unreadable,
            unfinished: BTreeMap::new(),
            sessions: BTreeMap::new(),
            notice: None,
        };
        tuning.recover();
        tuning
    }

    /// Shows every unfinished step. The files stay as they are until the
    /// user acts on that card, so a second crash before anyone saw the
    /// warning shows it again.
    fn recover(&mut self) {
        for uuid in in_progress(&self.journal, &self.marker) {
            let trial = self
                .journal
                .entries
                .get(&uuid)
                .map(|e| e.trial_w)
                .or_else(|| self.marker.get(&uuid).copied());
            let words = trial.map_or_else(
                || "a tuning step was in progress when the Hub last stopped".to_string(),
                |w| format!("a tuning step ({w} W) was in progress when the Hub last stopped"),
            );
            self.unfinished.insert(uuid, words);
        }
    }

    /// The user pressed Tune on a card with an unfinished step: that step is
    /// now recorded as bad and closed.
    fn acknowledge(&mut self, uuid: &str, dev: &GpuStat) -> Result<(), String> {
        if self.unfinished.remove(uuid).is_none() {
            return Ok(());
        }
        // The marker names the step even when the journal entry was lost or
        // edited, so it is the one recorded as bad.
        if let Some(trial_w) = self.marker.get(uuid).copied() {
            let old = self.journal.entries.get(uuid);
            let driver = old
                .map(|e| e.driver.clone())
                .or_else(|| dev.driver.clone())
                .unwrap_or_default();
            let default_w = old.map_or(dev.power_default_w.unwrap_or(0.0), |e| e.default_w);
            self.journal.entries.insert(
                uuid.to_string(),
                Entry {
                    driver,
                    default_w,
                    previous_w: trial_w,
                    trial_w,
                    state: EntryState::Trying,
                },
            );
        }
        self.store
            .end(&mut self.journal, &mut self.marker, uuid, EntryState::Bad)
    }

    /// True while any GPU is in a step. The supervisor reads telemetry then.
    #[must_use]
    pub fn active(&self) -> bool {
        self.sessions.values().any(|s| s.tuner.phase().active())
    }

    /// Turns guided tuning on or off for one GPU. Takes no watt value: every
    /// step comes from the driver's own range (ADR-137 R-1).
    pub fn set(
        &mut self,
        dev: &GpuStat,
        on: bool,
        floor_pct: f64,
        ceiling_c: f64,
    ) -> Result<(), String> {
        let uuid = dev
            .uuid
            .clone()
            .ok_or("the driver reports no UUID for this GPU, so it cannot be tuned")?;
        if !on {
            self.acknowledge(&uuid, dev)?;
            if let Some(s) = self.sessions.get_mut(&uuid) {
                let step = s.tuner.abort(Reason::UserStopped);
                self.record(&uuid, step);
            }
            return Ok(());
        }
        if self
            .sessions
            .get(&uuid)
            .is_some_and(|s| s.tuner.phase().active())
        {
            return Ok(());
        }
        let range = Range::new(dev.power_min_w, dev.power_default_w).ok_or(
            "the driver did not report this card's power range, so no step can be suggested",
        )?;
        let driver = dev
            .driver
            .clone()
            .ok_or("the driver did not report its version, so a result could not be kept")?;
        if self.untrusted.is_some() {
            // The user turned tuning on again: ADR-137 lets that start over.
            // Untrusted stays set until the new files are on disk.
            self.store.reset()?;
            (self.journal, self.marker) = (Journal::default(), Marker::new());
            self.untrusted = None;
        }
        self.acknowledge(&uuid, dev)?;
        let bad = self.journal.bad_at_or_below(&uuid, &driver);
        let tuner = Tuner::new(range, floor_pct, ceiling_c, bad);
        self.sessions.insert(
            uuid,
            Session {
                tuner,
                driver,
                hps_per_w: None,
            },
        );
        Ok(())
    }

    /// One supervision step for every session, matched to its card by UUID.
    pub fn tick(&mut self, workers: &[MinerState], devices: &[GpuStat]) {
        let uuids: Vec<String> = self.sessions.keys().cloned().collect();
        for uuid in uuids {
            let dev = devices
                .iter()
                .find(|d| d.uuid.as_deref() == Some(uuid.as_str()));
            let sample = sample_for(dev, workers);
            let Some(s) = self.sessions.get_mut(&uuid) else {
                continue;
            };
            s.hps_per_w = efficiency(sample.hps, sample.power_w);
            let step = s.tuner.on_sample(sample);
            self.record(&uuid, step);
        }
    }

    /// Writes what a step means to the journal and the profiles. A failed
    /// journal write stops the session before its step is shown.
    fn record(&mut self, uuid: &str, step: Step) {
        let Some(s) = self.sessions.get_mut(uuid) else {
            return;
        };
        let result = match step {
            Step::Hold => Ok(()),
            Step::Suggest {
                trial_w,
                previous_w,
            } => {
                let entry = Entry {
                    driver: s.driver.clone(),
                    default_w: s.tuner.range().default_w,
                    previous_w,
                    trial_w,
                    state: EntryState::Trying,
                };
                let written = self
                    .store
                    .begin(&mut self.journal, &mut self.marker, uuid, entry);
                if written.is_err() {
                    s.tuner.abort(Reason::JournalFailed);
                }
                written
            }
            Step::Finished { best_w, gain_pct } => {
                self.profiles.gpus.insert(
                    uuid.to_string(),
                    Profile {
                        driver: s.driver.clone(),
                        best_w,
                        floor_pct: s.tuner.floor_pct(),
                        gain_pct,
                    },
                );
                let ended = self.end(uuid, EntryState::Done);
                ended.and(self.save_profiles())
            }
            Step::Aborted(reason) => {
                let state = if reason.implicates_setting() {
                    EntryState::Bad
                } else {
                    EntryState::Done
                };
                self.end(uuid, state)
            }
        };
        if let Err(why) = result {
            self.notice = Some(format!("tuning record not saved: {why}"));
        }
    }

    /// Closes the open step of `uuid`, if it has one.
    fn end(&mut self, uuid: &str, state: EntryState) -> Result<(), String> {
        let open = self
            .journal
            .entries
            .get(uuid)
            .is_some_and(|e| e.state == EntryState::Trying);
        if open {
            self.store
                .end(&mut self.journal, &mut self.marker, uuid, state)
        } else {
            Ok(())
        }
    }

    fn save_profiles(&self) -> Result<(), String> {
        match &self.profiles_unreadable {
            // Never written over: it may hold another run's results.
            Some(why) => Err(why.clone()),
            None => profiles::save(&self.profile_path, &self.profiles),
        }
    }

    /// One line per GPU that has a session or an unfinished step. `devices`
    /// gives the live range and driver for each line.
    #[must_use]
    pub fn report(&self, devices: &[GpuStat]) -> Report {
        let mut views: Vec<TuneView> = self
            .sessions
            .iter()
            .map(|(uuid, s)| {
                let (line, set_w) =
                    instruction(s.tuner.phase(), s.tuner.range(), s.tuner.ceiling_c());
                TuneView {
                    uuid: uuid.clone(),
                    on: s.tuner.phase().active(),
                    line,
                    set_w,
                    hps_per_w: s.hps_per_w,
                    floor_pct: Some(s.tuner.floor_pct()),
                    saved_w: self.saved(uuid, devices),
                }
            })
            .collect();
        for (uuid, words) in &self.unfinished {
            let default = devices
                .iter()
                .find(|d| d.uuid.as_deref() == Some(uuid.as_str()))
                .and_then(|d| d.power_default_w)
                .map(f64::round);
            let line = default.map_or_else(
                || format!("Stopped: {words}. Set the power limit back to the driver default."),
                |d| format!("Stopped: {words}. Set the power limit back to the default, {d} W."),
            );
            views.push(TuneView {
                uuid: uuid.clone(),
                on: false,
                line,
                set_w: default,
                hps_per_w: None,
                floor_pct: None,
                saved_w: None,
            });
        }
        let notice = self
            .untrusted
            .as_ref()
            .map(|why| {
                format!(
                    "The tuning journal could not be trusted ({why}). Set any card you tuned \
                     back to its default power limit. Turning Tune on again starts a new journal."
                )
            })
            .or_else(|| self.profiles_unreadable.clone())
            .or_else(|| self.notice.clone());
        Report {
            devices: views,
            notice,
        }
    }

    fn saved(&self, uuid: &str, devices: &[GpuStat]) -> Option<f64> {
        let dev = devices.iter().find(|d| d.uuid.as_deref() == Some(uuid))?;
        let range = Range::new(dev.power_min_w, dev.power_default_w)?;
        self.profiles.hint(uuid, dev.driver.as_deref()?, range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::tests::{entry, scratch};
    use crate::journal::{JOURNAL_FILE, MARKER_FILE};
    use crate::miner::RateSample;
    use crate::sim_gpu::{SIM_RANGE, SimGpu};

    const DRIVER: &str = "581.29";

    /// The driver's view of the SIM card at index `index`.
    fn dev(uuid: &str, index: u32, sim: &SimGpu) -> GpuStat {
        let s = sim.sample();
        GpuStat {
            index,
            uuid: Some(uuid.into()),
            power_w: s.power_w,
            power_limit_w: s.limit_w,
            temp_c: s.temp_c,
            power_default_w: Some(SIM_RANGE.default_w),
            power_min_w: Some(SIM_RANGE.min_w),
            driver: Some(DRIVER.into()),
            ..GpuStat::default()
        }
    }

    fn worker(index: u32, sim: &SimGpu) -> MinerState {
        let mut w = MinerState::default();
        w.device = device_key(Some(index));
        w.running = true;
        let hps = sim.sample().hps.unwrap_or_default();
        w.rates.push_back(RateSample { t_ms: 0, hps });
        w
    }

    /// Runs the guided flow with the test as the user, until the search ends.
    fn guide(tuning: &mut Tuning, sim: &mut SimGpu) -> TuneView {
        for _ in 0..2_000 {
            tuning.tick(&[worker(0, sim)], &[dev("GPU-a", 0, sim)]);
            let view = tuning.report(&[dev("GPU-a", 0, sim)]).devices.remove(0);
            if !view.on {
                return view;
            }
            if let Some(w) = view.set_w {
                sim.set_limit(w);
            }
        }
        panic!("the guided search must end");
    }

    #[test]
    fn the_guided_flow_ends_with_a_kept_limit_and_a_saved_profile() {
        let dir = scratch("flow");
        let mut sim = SimGpu::new();
        let mut tuning = Tuning::open(&dir);
        tuning
            .set(&dev("GPU-a", 0, &sim), true, 95.0, 80.0)
            .expect("on");
        let end = guide(&mut tuning, &mut sim);
        assert_eq!(end.set_w, Some(220.0), "{}", end.line);
        assert!(
            end.line.contains("keep the power limit at 220 W"),
            "{}",
            end.line
        );
        assert!(!dir.join(MARKER_FILE).exists(), "no step left open");

        let reopened = Tuning::open(&dir);
        assert!(reopened.unfinished.is_empty());
        assert_eq!(
            reopened.saved("GPU-a", &[dev("GPU-a", 0, &sim)]),
            Some(220.0)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_tuner_command_takes_no_watt_value_from_the_caller() {
        // `set` has no watt parameter; the first step is computed from the
        // driver's range, and a hostile floor is clamped to the slider's range.
        let dir = scratch("nowatts");
        let sim = SimGpu::new();
        let mut tuning = Tuning::open(&dir);
        tuning
            .set(&dev("GPU-a", 0, &sim), true, -1e9, 80.0)
            .expect("on");
        let view = &tuning.report(&[]).devices[0];
        assert_eq!(view.floor_pct, Some(crate::autotune::FLOOR_PCT_MIN));
        assert_eq!(view.set_w, Some(SIM_RANGE.default_w));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_trial_left_in_trying_state_is_rolled_back_and_marked_bad_on_start() {
        let dir = scratch("crash");
        let store = Store::new(&dir);
        let (mut j, mut m) = (Journal::default(), Marker::new());
        store
            .begin(&mut j, &mut m, "GPU-a", entry(205.0))
            .expect("begin");
        // The Hub dies here, and again before anyone saw the warning.
        drop(Tuning::open(&dir));
        let mut tuning = Tuning::open(&dir);
        let sim = SimGpu::new();
        let view = tuning.report(&[dev("GPU-a", 0, &sim)]).devices.remove(0);
        assert!(!view.on, "tuning stays off");
        assert!(
            view.line.contains("205 W") && view.line.contains("250 W"),
            "{}",
            view.line
        );
        assert_eq!(view.set_w, Some(250.0), "back to the live driver default");
        assert!(dir.join(MARKER_FILE).exists(), "kept until the user acts");
        // The user presses Tune: the step is recorded bad and never suggested again.
        tuning
            .set(&dev("GPU-a", 0, &sim), true, 95.0, 80.0)
            .expect("on again");
        assert_eq!(tuning.journal.bad_at_or_below("GPU-a", DRIVER), Some(205.0));
        assert!(!dir.join(MARKER_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_or_missing_journal_is_treated_as_a_trial_in_progress() {
        let dir = scratch("missing");
        std::fs::create_dir_all(&dir).expect("dir");
        // The marker survives and the journal was deleted.
        crate::journal::write_synced(
            &dir.join(MARKER_FILE),
            &Marker::from([("GPU-a".to_string(), 235.0)]),
        )
        .expect("seed");
        let mut tuning = Tuning::open(&dir);
        assert!(tuning.unfinished.contains_key("GPU-a"));
        let sim = SimGpu::new();
        tuning
            .set(&dev("GPU-a", 0, &sim), false, 95.0, 80.0)
            .expect("acknowledge");
        assert_eq!(
            tuning.journal.bad_at_or_below("GPU-a", DRIVER),
            Some(235.0),
            "the marker alone still marks the step bad"
        );

        std::fs::write(dir.join(JOURNAL_FILE), "{ not json").expect("seed");
        let mut tuning = Tuning::open(&dir);
        let notice = tuning.report(&[]).notice.unwrap_or_default();
        assert!(notice.contains("could not be trusted"), "{notice}");
        // Turning it on again is the user's explicit act, and starts over.
        tuning
            .set(&dev("GPU-b", 1, &sim), true, 95.0, 80.0)
            .expect("on again");
        assert!(tuning.report(&[]).notice.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_journal_written_for_one_gpu_never_restores_onto_another() {
        let dir = scratch("other");
        let store = Store::new(&dir);
        let (mut j, mut m) = (Journal::default(), Marker::new());
        store
            .begin(&mut j, &mut m, "GPU-a", entry(205.0))
            .expect("begin");
        let mut tuning = Tuning::open(&dir);
        let sim = SimGpu::new();
        let devices = [dev("GPU-b", 0, &sim), dev("GPU-a", 1, &sim)];
        let report = tuning.report(&devices);
        assert_eq!(report.devices.len(), 1);
        assert_eq!(report.devices[0].uuid, "GPU-a");
        assert_eq!(tuning.journal.bad_at_or_below("GPU-b", DRIVER), None);
        tuning
            .set(&devices[0], true, 95.0, 80.0)
            .expect("GPU-b tunes normally");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_card_is_targeted_by_uuid_not_by_index() {
        let dir = scratch("uuid");
        let sim = SimGpu::new();
        let mut tuning = Tuning::open(&dir);
        tuning
            .set(&dev("GPU-a", 0, &sim), true, 95.0, 80.0)
            .expect("on");
        for _ in 0..5 {
            tuning.tick(&[worker(1, &sim)], &[dev("GPU-a", 1, &sim)]);
        }
        // The cards swap indices: GPU-a is now at index 1 and running hot.
        let mut hot = dev("GPU-a", 1, &sim);
        hot.temp_c = Some(79.0);
        tuning.tick(
            &[worker(0, &sim), worker(1, &sim)],
            &[dev("GPU-b", 0, &sim), hot],
        );
        let view = &tuning.report(&[]).devices[0];
        assert!(view.line.contains("temperature limit"), "{}", view.line);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stopped_session_tells_the_user_to_go_back_to_the_default() {
        let dir = scratch("stop");
        let mut sim = SimGpu::new();
        let mut tuning = Tuning::open(&dir);
        tuning
            .set(&dev("GPU-a", 0, &sim), true, 95.0, 80.0)
            .expect("on");
        for _ in 0..40 {
            tuning.tick(&[worker(0, &sim)], &[dev("GPU-a", 0, &sim)]);
            if let Some(w) = tuning.report(&[]).devices[0].set_w {
                sim.set_limit(w);
            }
        }
        assert!(dir.join(MARKER_FILE).exists(), "a step is open");
        tuning
            .set(&dev("GPU-a", 0, &sim), false, 95.0, 80.0)
            .expect("off");
        let view = &tuning.report(&[]).devices[0];
        assert!(!view.on);
        assert!(
            view.line.contains("back to the default, 250 W"),
            "{}",
            view.line
        );
        assert!(
            !dir.join(MARKER_FILE).exists(),
            "the step is closed, not marked bad"
        );
        assert_eq!(tuning.journal.bad_at_or_below("GPU-a", DRIVER), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
