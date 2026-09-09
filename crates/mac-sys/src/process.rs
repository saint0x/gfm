use std::mem::MaybeUninit;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeProcessMemory {
    pub status: NativeProcessMemoryStatus,
    pub peak_resident_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeProcessMemoryStatus {
    Available,
    Unavailable,
}

pub fn copy_process_memory() -> NativeProcessMemory {
    let mut usage = MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the provided rusage buffer when it returns 0.
    // The pointer is valid for writes and lives for the duration of the call.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status != 0 {
        return NativeProcessMemory {
            status: NativeProcessMemoryStatus::Unavailable,
            peak_resident_bytes: 0,
        };
    }
    // SAFETY: status == 0 proves getrusage initialized the rusage value.
    let usage = unsafe { usage.assume_init() };
    NativeProcessMemory {
        status: NativeProcessMemoryStatus::Available,
        peak_resident_bytes: usage.ru_maxrss.try_into().unwrap_or(0),
    }
}
