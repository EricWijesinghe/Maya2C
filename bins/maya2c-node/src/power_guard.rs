//! Start-up guard for validator mode on Windows.
//!
//! A validator whose PC sleeps misses its turns, and Modern Standby sleeps a
//! PC on the active plan's idle timers. So a validator reads the AC plan and
//! refuses to start while either timer can fire, unless `--allow-sleep` says
//! otherwise. Operational only: nothing here reaches consensus.

use std::error::Error;

#[cfg(windows)]
use windows_sys::{
    Win32::{
        Foundation::LocalFree,
        System::{
            Power::{
                ES_CONTINUOUS, ES_SYSTEM_REQUIRED, PowerGetActiveScheme, PowerReadACValueIndex,
                SetThreadExecutionState,
            },
            SystemServices::{GUID_HIBERNATE_TIMEOUT, GUID_SLEEP_SUBGROUP, GUID_STANDBY_TIMEOUT},
        },
    },
    core::GUID,
};

/// AC idle timers of the active power plan, in seconds. Zero means never.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SleepSettings {
    standby_ac_secs: u32,
    hibernate_ac_secs: u32,
}

/// Why this plan may put the PC to sleep, or `None` when it may not.
#[cfg(any(windows, test))]
fn sleep_risk(read: &Result<SleepSettings, String>) -> Option<String> {
    match read {
        Err(error) => Some(format!("the AC power plan could not be read ({error})")),
        Ok(SleepSettings {
            standby_ac_secs: 0,
            hibernate_ac_secs: 0,
        }) => None,
        Ok(settings) => Some(format!(
            "the AC power plan sleeps this PC (standby after {} s, hibernate after {} s; 0 means never)",
            settings.standby_ac_secs, settings.hibernate_ac_secs
        )),
    }
}

/// The refusal text, naming the exact commands that make the PC stay awake.
#[cfg(any(windows, test))]
fn refusal_message(reason: &str) -> String {
    format!(
        "refusing to start a validator: {reason}.\n\
         A sleeping validator misses its turns. Stop the sleep from an elevated shell:\n  \
         powercfg /change standby-timeout-ac 0\n  \
         powercfg /change hibernate-timeout-ac 0\n\
         or pass --allow-sleep to start anyway."
    )
}

/// Refuses a validator on a power plan that can sleep the PC.
///
/// Non-validator runs and non-Windows hosts pass straight through: this guard
/// knows only the Windows power plan.
#[cfg(not(windows))]
pub(crate) fn guard(_validator: bool, _allow_sleep: bool) -> Result<(), Box<dyn Error>> {
    Ok(())
}

/// Refuses a validator on a power plan that can sleep the PC, and keeps the
/// PC awake while it runs when the plan allows it.
#[cfg(windows)]
pub(crate) fn guard(validator: bool, allow_sleep: bool) -> Result<(), Box<dyn Error>> {
    if !validator {
        return Ok(());
    }
    match sleep_risk(&read_ac_settings()) {
        None => {
            hold_awake();
            Ok(())
        }
        Some(reason) if allow_sleep => {
            eprintln!("warning: {reason}; starting anyway (--allow-sleep)");
            Ok(())
        }
        Some(reason) => Err(refusal_message(&reason).into()),
    }
}

/// Reads the AC standby and hibernate timers of the active plan.
///
/// The Win32 call is used rather than parsing `powercfg /query`, whose labels
/// are translated: a German or Japanese Windows would fail a text match.
#[cfg(windows)]
fn read_ac_settings() -> Result<SleepSettings, String> {
    let scheme = ActiveScheme::get()?;
    Ok(SleepSettings {
        standby_ac_secs: read_ac_value(scheme.guid(), &GUID_STANDBY_TIMEOUT)?,
        hibernate_ac_secs: read_ac_value(scheme.guid(), &GUID_HIBERNATE_TIMEOUT)?,
    })
}

/// The GUID of the active power scheme, freed with `LocalFree` on drop.
#[cfg(windows)]
struct ActiveScheme(*mut GUID);

#[cfg(windows)]
impl ActiveScheme {
    fn get() -> Result<Self, String> {
        let mut guid: *mut GUID = std::ptr::null_mut();
        // SAFETY: a null root key selects the current user's plan store. On
        // success `guid` is a buffer the caller frees; on failure it is left null.
        let status = unsafe { PowerGetActiveScheme(std::ptr::null_mut(), &raw mut guid) };
        if status == 0 && !guid.is_null() {
            Ok(Self(guid))
        } else {
            Err(format!("PowerGetActiveScheme failed with status {status}"))
        }
    }

    fn guid(&self) -> &GUID {
        // SAFETY: `self.0` is non-null and owned by `self` until drop.
        unsafe { &*self.0 }
    }
}

#[cfg(windows)]
impl Drop for ActiveScheme {
    fn drop(&mut self) {
        // SAFETY: the buffer came from PowerGetActiveScheme and is freed once here.
        unsafe { LocalFree(self.0.cast()) };
    }
}

#[cfg(windows)]
fn read_ac_value(scheme: &GUID, setting: &GUID) -> Result<u32, String> {
    let mut value = 0u32;
    // SAFETY: a null root key is the documented default; the scheme, subgroup
    // and setting GUIDs and `value` are live for the call, which keeps none of them.
    let status = unsafe {
        PowerReadACValueIndex(
            std::ptr::null_mut(),
            scheme,
            &GUID_SLEEP_SUBGROUP,
            setting,
            &raw mut value,
        )
    };
    if status == 0 {
        Ok(value)
    } else {
        Err(format!("PowerReadACValueIndex failed with status {status}"))
    }
}

/// Asks Windows to keep the system awake for the calling thread. Returns the
/// previous execution state, which is zero when the request failed.
#[cfg(windows)]
fn request_awake() -> u32 {
    // SAFETY: the call takes a flag word and no pointers. The request lasts
    // while the calling thread lives, which is why `hold_awake` parks its own.
    unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) }
}

/// Holds the system awake for the life of the process. The request is per
/// thread, so a thread is parked here to keep it.
#[cfg(windows)]
fn hold_awake() {
    let spawned = std::thread::Builder::new()
        .name("power-hold".into())
        .spawn(|| {
            if request_awake() == 0 {
                eprintln!("warning: could not keep this PC awake while validating");
            }
            loop {
                std::thread::park();
            }
        });
    if let Err(error) = spawned {
        eprintln!("warning: could not start the keep-awake thread: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEVER: SleepSettings = SleepSettings {
        standby_ac_secs: 0,
        hibernate_ac_secs: 0,
    };

    #[test]
    fn a_plan_that_never_sleeps_passes() {
        assert_eq!(sleep_risk(&Ok(NEVER)), None);
    }

    #[test]
    fn a_standby_timer_is_a_risk() {
        let settings = SleepSettings {
            standby_ac_secs: 1800,
            hibernate_ac_secs: 0,
        };
        let reason = sleep_risk(&Ok(settings)).expect("standby 1800 s is a risk");
        assert!(reason.contains("standby after 1800 s"), "{reason}");
    }

    #[test]
    fn a_hibernate_timer_is_a_risk() {
        let settings = SleepSettings {
            standby_ac_secs: 0,
            hibernate_ac_secs: 3600,
        };
        let reason = sleep_risk(&Ok(settings)).expect("hibernate 3600 s is a risk");
        assert!(reason.contains("hibernate after 3600 s"), "{reason}");
    }

    #[test]
    fn an_unreadable_plan_is_refused_not_assumed_safe() {
        let reason = sleep_risk(&Err("status 2".into())).expect("unknown is a risk");
        assert!(reason.contains("could not be read"), "{reason}");
    }

    #[test]
    fn the_refusal_names_both_fix_commands_and_the_override() {
        let message = refusal_message("the AC power plan sleeps this PC");
        assert!(
            message.contains("powercfg /change standby-timeout-ac 0"),
            "{message}"
        );
        assert!(
            message.contains("powercfg /change hibernate-timeout-ac 0"),
            "{message}"
        );
        assert!(message.contains("--allow-sleep"), "{message}");
    }

    #[cfg(windows)]
    #[test]
    fn the_os_plan_is_readable() {
        // Live call into powrprof: the FFI is only exercised by a real read.
        assert!(read_ac_settings().is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn the_keep_awake_request_is_accepted() {
        // The first call sets the state; the second returns that previous
        // state, which is non-zero only if the first call took effect.
        request_awake();
        assert_ne!(request_awake(), 0);
    }
}
