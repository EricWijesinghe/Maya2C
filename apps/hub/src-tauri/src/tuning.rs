//! Hub GPU auto-tuner, the driver (ADR-137, HW-1). It owns one session per
//! GPU the user turned Tune on for, carries out the tuner's steps through the
//! vendor backend, and keeps the profile file. Off unless the user asks for it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::Serialize;

use crate::autotune::{Phase, Reason, Sample, Step, Tuner};
use crate::gpu::GpuStat;
use crate::gpu_control::{ControlError, GpuControl, Knobs, NvidiaSmi, ReadOnly};
use crate::miner::{MinerState, device_key};
use crate::profiles::{self, Profile, ProfileFile};

const PROFILE_FILE: &str = "gpu-tuning.json";
/// The lowest ceiling the settings page allows (50 C).
const MIN_CEILING_C: f64 = 50.0;

/// One GPU under tuning, or being reset after a rollback.
pub struct Session {
    uuid: String,
    control: Box<dyn GpuControl>,
    tuner: Tuner,
    stock: Knobs,
    /// The reset to stock failed; it is retried every tick until it works.
    reset_pending: bool,
    /// The last driver error, shown to the user.
    note: Option<String>,
}

/// Every GPU's tuning state, behind one mutex shared with the supervisor.
pub struct Tuning {
    path: PathBuf,
    file: ProfileFile,
    /// The profile file could not be read. Nothing is written over it.
    unreadable: Option<String>,
    /// A problem outside any one GPU, such as a failed save.
    notice: Option<String>,
    sessions: BTreeMap<u32, Session>,
}

/// One GPU's line on the rig card.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TuneView {
    pub index: u32,
    pub on: bool,
    pub line: String,
    pub gain_pct: Option<f64>,
    pub trials: u32,
    pub backend: &'static str,
    pub note: Option<String>,
}

/// Everything the tuning UI shows.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    pub devices: Vec<TuneView>,
    pub notice: Option<String>,
}

/// Locks the tuning state, recovering from a poisoned mutex. The state is
/// plain data, so a panic elsewhere cannot leave it half-written.
pub fn lock(state: &Mutex<Tuning>) -> MutexGuard<'_, Tuning> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The words for one GPU's state. `step` is the trial number.
#[must_use]
pub fn describe(phase: Phase, step: u32, gain_pct: Option<f64>) -> String {
    match phase {
        Phase::Waiting => "stock, waiting for the miner to hash".into(),
        Phase::Baseline => "stock, measuring".into(),
        Phase::Trial(_) => format!("tuning step {step}"),
        Phase::Tuned => match gain_pct {
            Some(g) if g > 0.0 => format!("tuned +{g:.1}% efficiency"),
            _ => "tuned, stock is already the best found".into(),
        },
        Phase::RolledBack(reason) => format!("rolled back: {}", reason.words()),
    }
}

/// The backend for one GPU. A device with no UUID cannot be keyed, so it
/// cannot be tuned.
fn backend_for(dev: &GpuStat) -> Box<dyn GpuControl> {
    match dev.uuid {
        Some(_) => Box::new(NvidiaSmi::new(dev.index)),
        None => Box::new(ReadOnly::new("the driver reports no device UUID")),
    }
}

/// This GPU's reading for the tuner. `None` when the device or its power or
/// temperature is missing: no reading, no decision.
fn sample_for(index: u32, workers: &[MinerState], dev: &GpuStat) -> Option<Sample> {
    let key = device_key(Some(index));
    let worker = workers.iter().find(|w| w.device == key);
    Some(Sample {
        hps: worker.and_then(|w| w.rates.back()).map_or(0.0, |r| r.hps),
        power_w: dev.power_w?,
        temp_c: dev.temp_c?,
        running: worker.is_some_and(|w| w.running),
        restarts: worker.map_or(0, |w| w.restarts),
    })
}

/// Restores stock. True when the profile file changed (the GPU is no longer
/// active).
fn reset_to_stock(session: &mut Session, file: &mut ProfileFile) -> bool {
    match session.control.reset(session.stock) {
        Ok(()) => {
            session.reset_pending = false;
            file.active.remove(&session.uuid).is_some()
        }
        Err(e) => {
            session.reset_pending = true;
            session.note = Some(format!("reset to stock failed, retrying: {e}"));
            false
        }
    }
}

/// A refused write ends the session. The tuner rolls back and the device is reset.
fn fail(session: &mut Session, e: ControlError, file: &mut ProfileFile) -> bool {
    session.note = Some(e.to_string());
    let _ = session.tuner.stop(Reason::ControlError);
    reset_to_stock(session, file)
}

/// Carries out one tuner step. True when the profile file changed.
fn act(session: &mut Session, step: Step, file: &mut ProfileFile) -> bool {
    match step {
        Step::Hold => false,
        Step::Rollback(_) => reset_to_stock(session, file),
        Step::Apply(k) => match session.control.apply(k) {
            Ok(()) => false,
            Err(e) => fail(session, e, file),
        },
        Step::Settle(k) => match session.control.apply(k) {
            Ok(()) => {
                let gain_pct = session.tuner.gain_pct().unwrap_or(0.0);
                let profile = Profile {
                    stock: session.stock,
                    best: k,
                    gain_pct,
                };
                file.gpus.insert(session.uuid.clone(), profile);
                true
            }
            Err(e) => fail(session, e, file),
        },
    }
}

impl Tuning {
    /// Opens the profile file in the Hub's data folder. An unreadable file
    /// turns tuning off, so nothing is written over it.
    #[must_use]
    pub fn open(dir: &Path) -> Self {
        let path = dir.join(PROFILE_FILE);
        let (file, unreadable) = match profiles::load(&path) {
            Ok(file) => (file, None),
            Err(why) => (ProfileFile::default(), Some(why)),
        };
        Self {
            path,
            file,
            unreadable,
            notice: None,
            sessions: BTreeMap::new(),
        }
    }

    /// True while a GPU is tuned, resetting, or left tuned by an earlier run.
    /// The supervisor reads GPU telemetry only while this holds.
    #[must_use]
    pub fn active(&self) -> bool {
        !self.file.active.is_empty()
            || self
                .sessions
                .values()
                .any(|s| s.reset_pending || !matches!(s.tuner.phase(), Phase::RolledBack(_)))
    }

    /// Turns tuning on or off for one GPU.
    pub fn set(&mut self, dev: &GpuStat, on: bool, ceiling_c: f64) -> Result<(), String> {
        if !on {
            return self.stop_device(dev.index);
        }
        if let Some(why) = &self.unreadable {
            return Err(format!(
                "the profile file cannot be read, so tuning is refused: {why}"
            ));
        }
        if let Some(s) = self.sessions.get(&dev.index) {
            if !matches!(s.tuner.phase(), Phase::RolledBack(_)) {
                return Ok(());
            }
            if s.reset_pending {
                return Err(
                    "the last tune is still being reset to stock; try again shortly".into(),
                );
            }
        }
        let uuid = dev
            .uuid
            .clone()
            .ok_or("the driver reports no UUID for this GPU, so its profile cannot be kept")?;
        if self.file.active.contains_key(&uuid) {
            return Err(
                "an earlier tune of this GPU is still being reset; try again shortly".into(),
            );
        }
        let control = backend_for(dev);
        let stock = control.stock().map_err(|e| e.to_string())?;
        let seed = self.file.gpus.get(&uuid).map(|p| p.best);
        let tuner = Tuner::new(stock, ceiling_c.max(MIN_CEILING_C), seed);
        self.file.active.insert(uuid.clone(), stock);
        if let Err(why) = self.save() {
            self.file.active.remove(&uuid);
            return Err(why);
        }
        self.sessions.insert(
            dev.index,
            Session {
                uuid,
                control,
                tuner,
                stock,
                reset_pending: false,
                note: None,
            },
        );
        Ok(())
    }

    fn stop_device(&mut self, index: u32) -> Result<(), String> {
        let Some(session) = self.sessions.get_mut(&index) else {
            return Ok(());
        };
        let step = session.tuner.stop(Reason::UserTurnedOff);
        let changed = act(session, step, &mut self.file);
        let done = !session.reset_pending;
        let note = session.note.clone();
        if done {
            self.sessions.remove(&index);
        }
        if changed {
            self.persist();
        }
        if done {
            Ok(())
        } else {
            Err(note.unwrap_or_else(|| "reset to stock failed; retrying".into()))
        }
    }

    /// Runs one supervision step for every tuned GPU. `workers` are the miner's
    /// workers and `devices` the latest GPU readings.
    pub fn tick(&mut self, workers: &[MinerState], devices: &[GpuStat]) {
        let mut dirty = self.recover(devices);
        let indices: Vec<u32> = self.sessions.keys().copied().collect();
        for index in indices {
            let Some(session) = self.sessions.get_mut(&index) else {
                continue;
            };
            if session.reset_pending {
                dirty |= reset_to_stock(session, &mut self.file);
                continue;
            }
            let Some(dev) = devices.iter().find(|d| d.index == index) else {
                continue;
            };
            let step = sample_for(index, workers, dev)
                .map_or(Step::Hold, |sample| session.tuner.on_sample(sample));
            dirty |= act(session, step, &mut self.file);
        }
        if dirty {
            self.persist();
        }
    }

    /// Resets every GPU on the way out. A failed reset stays `active`, so the
    /// next start resets it.
    pub fn reset_all(&mut self) {
        let mut changed = false;
        for session in self.sessions.values_mut() {
            let step = session.tuner.stop(Reason::AppExit);
            changed |= act(session, step, &mut self.file);
        }
        if changed {
            self.persist();
        }
    }

    /// Resets GPUs an earlier run left tuned (a crash or a power cut) and that
    /// are visible now. True when the profile file changed.
    fn recover(&mut self, devices: &[GpuStat]) -> bool {
        let stale: Vec<(String, Knobs)> = self
            .file
            .active
            .iter()
            .filter(|(uuid, _)| !self.sessions.values().any(|s| s.uuid == **uuid))
            .map(|(uuid, stock)| (uuid.clone(), *stock))
            .collect();
        let mut changed = false;
        for (uuid, stock) in stale {
            let Some(dev) = devices
                .iter()
                .find(|d| d.uuid.as_deref() == Some(uuid.as_str()))
            else {
                continue;
            };
            match backend_for(dev).reset(stock) {
                Ok(()) => changed |= self.file.active.remove(&uuid).is_some(),
                Err(e) => {
                    self.notice = Some(format!(
                        "GPU {} was left tuned by an earlier run; reset failed, retrying: {e}",
                        dev.index
                    ));
                }
            }
        }
        changed
    }

    /// One card's line for each GPU that has a session.
    #[must_use]
    pub fn views(&self) -> Vec<TuneView> {
        self.sessions
            .iter()
            .map(|(index, s)| TuneView {
                index: *index,
                on: !matches!(s.tuner.phase(), Phase::RolledBack(_)),
                line: describe(s.tuner.phase(), s.tuner.trials(), s.tuner.gain_pct()),
                gain_pct: s.tuner.gain_pct(),
                trials: s.tuner.trials(),
                backend: s.control.backend(),
                note: s.note.clone(),
            })
            .collect()
    }

    #[must_use]
    pub fn report(&self) -> Report {
        let notice = self.unreadable.clone().or_else(|| self.notice.clone());
        Report {
            devices: self.views(),
            notice,
        }
    }

    fn save(&self) -> Result<(), String> {
        if let Some(why) = &self.unreadable {
            return Err(why.clone());
        }
        profiles::save(&self.path, &self.file)
    }

    /// Saves, and records a failure as the notice rather than dropping it.
    fn persist(&mut self) {
        if let Err(why) = self.save() {
            self.notice = Some(format!("the tuning profile was not saved: {why}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::miner::RateSample;
    use crate::sim_gpu::{SIM_STOCK, SimGpu};

    fn dev() -> GpuStat {
        GpuStat {
            index: 0,
            uuid: Some("GPU-a".into()),
            power_w: Some(225.0),
            temp_c: Some(71.0),
            ..GpuStat::default()
        }
    }

    fn worker(running: bool, restarts: u32, hps: f64) -> MinerState {
        let mut w = MinerState::default();
        w.device = "gpu0".into();
        w.running = running;
        w.restarts = restarts;
        w.rates.push_back(RateSample { t_ms: 0, hps });
        w
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hub-tune-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A tuning state with one SIM session for GPU 0, already marked active.
    fn with_sim_session(dir: &Path) -> Tuning {
        let mut tuning = Tuning::open(dir);
        let tuner = Tuner::new(SIM_STOCK, 80.0, None);
        tuning.sessions.insert(
            0,
            Session {
                uuid: "GPU-a".into(),
                control: Box::new(SimGpu::new()),
                tuner,
                stock: SIM_STOCK,
                reset_pending: false,
                note: None,
            },
        );
        tuning.file.active.insert("GPU-a".into(), SIM_STOCK);
        tuning
    }

    #[test]
    fn the_words_match_each_state() {
        assert_eq!(describe(Phase::Baseline, 0, None), "stock, measuring");
        assert_eq!(
            describe(Phase::Trial(SIM_STOCK), 3, Some(4.0)),
            "tuning step 3"
        );
        assert_eq!(
            describe(Phase::Tuned, 9, Some(12.34)),
            "tuned +12.3% efficiency"
        );
        assert_eq!(
            describe(Phase::RolledBack(Reason::HashrateCollapse), 9, None),
            "rolled back: hash rate collapsed"
        );
    }

    #[test]
    fn a_stopped_miner_rolls_the_gpu_back_and_releases_it() {
        let dir = scratch("stop");
        let mut tuning = with_sim_session(&dir);
        // Five ticks: the start, a settle sample, then a three-sample baseline,
        // which is when the first trial is applied to the SIM card.
        for _ in 0..5 {
            tuning.tick(&[worker(true, 0, 100.0)], &[dev()]);
        }
        assert_eq!(tuning.views()[0].line, "tuning step 1");

        tuning.tick(&[worker(false, 0, 0.0)], &[dev()]);

        assert_eq!(tuning.views()[0].line, "rolled back: the miner stopped");
        assert!(
            !tuning.file.active.contains_key("GPU-a"),
            "a reset GPU is no longer active"
        );
        assert!(
            !tuning.active(),
            "a rolled-back GPU does not keep telemetry running"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_reading_means_no_decision() {
        let dir = scratch("noreading");
        let mut tuning = with_sim_session(&dir);
        for _ in 0..5 {
            tuning.tick(&[worker(true, 0, 100.0)], &[dev()]);
        }
        let blind = GpuStat {
            power_w: None,
            ..dev()
        };
        tuning.tick(&[worker(false, 0, 0.0)], &[blind]);
        assert_eq!(
            tuning.views()[0].line,
            "tuning step 1",
            "a GPU without power data is left alone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_profile_survives_a_restart_and_seeds_the_next_search() {
        let dir = scratch("restart");
        let mut tuning = Tuning::open(&dir);
        let best = Knobs {
            power_w: 150.0,
            clock_mhz: Some(1890.0),
        };
        tuning.file.gpus.insert(
            "GPU-a".into(),
            Profile {
                stock: SIM_STOCK,
                best,
                gain_pct: 78.6,
            },
        );
        tuning.persist();

        let reopened = Tuning::open(&dir);
        assert_eq!(reopened.file.gpus["GPU-a"].best, best);
        assert!(reopened.report().notice.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
