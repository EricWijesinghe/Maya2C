//! Process working-set size in bytes, from the OS.
//!
//! Windows only, through `psapi`, the same call `crates/node/tests/cert_replay_measure.rs`
//! makes. Elsewhere it reports 0 and the run says so rather than printing a
//! number that was never measured.

#[cfg(windows)]
mod imp {
    #[repr(C)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }

    /// The current process's working set, or 0 if the OS refuses.
    pub fn now() -> usize {
        let mut counters = Counters {
            cb: 0,
            page_fault_count: 0,
            peak_working_set_size: 0,
            working_set_size: 0,
            quota_peak_paged_pool_usage: 0,
            quota_paged_pool_usage: 0,
            quota_peak_non_paged_pool_usage: 0,
            quota_non_paged_pool_usage: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
        };
        counters.cb = u32::try_from(std::mem::size_of::<Counters>()).unwrap_or(0);
        // SAFETY: `counters` is a live, correctly sized PROCESS_MEMORY_COUNTERS
        // for this call; -1 is the documented pseudo-handle of the current
        // process and needs no CloseHandle.
        let ok = unsafe { GetProcessMemoryInfo(-1, &raw mut counters, counters.cb) };
        if ok == 0 {
            return 0;
        }
        counters.working_set_size
    }
}

#[cfg(not(windows))]
mod imp {
    /// Not measured off Windows.
    pub fn now() -> usize {
        0
    }
}

pub use imp::now;
