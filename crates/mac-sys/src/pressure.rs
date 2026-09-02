use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFGetTypeID, CFRelease, CFTypeRef};
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::string::{CFStringGetTypeID, CFStringRef};
use objc::runtime::{Class, Object, Sel};
use std::ffi::c_void;

#[link(name = "Foundation", kind = "framework")]
extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {}

#[link(name = "IOKit", kind = "framework")]
extern "C" {}

#[link(name = "objc")]
extern "C" {
    fn objc_msgSend();
}

extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFArrayRef;
    fn IOPSGetPowerSourceDescription(blob: CFTypeRef, ps: CFTypeRef) -> CFDictionaryRef;
    fn CFDictionaryGetValue(dictionary: CFDictionaryRef, key: *const c_void) -> *const c_void;
    fn CGEventSourceSecondsSinceLastEventType(state_id: u32, event_type: u32) -> f64;
}

const CG_EVENT_SOURCE_STATE_COMBINED_SESSION_STATE: u32 = 0;
const CG_ANY_INPUT_EVENT_TYPE: u32 = u32::MAX;
const ACTIVE_INPUT_WINDOW_SECONDS: f64 = 2.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeHostPressure {
    pub thermal_status: NativeHostSignalStatus,
    pub thermal_state: Option<NativeThermalState>,
    pub thermal_reason: Option<String>,
    pub low_power_status: NativeHostSignalStatus,
    pub low_power_enabled: Option<bool>,
    pub low_power_reason: Option<String>,
    pub power_source_status: NativeHostSignalStatus,
    pub power_source_state: Option<NativePowerSourceState>,
    pub power_source_reason: Option<String>,
    pub user_activity_status: NativeHostSignalStatus,
    pub user_activity_state: Option<NativeUserActivityState>,
    pub user_activity_idle_millis: Option<u64>,
    pub user_activity_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeHostSignalStatus {
    Available,
    Unsupported,
    Unavailable,
}

impl NativeHostSignalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unsupported => "unsupported",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePowerSourceState {
    AcPower,
    BatteryPower,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeUserActivityState {
    Idle,
    Active,
}

impl NativeUserActivityState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Active => "active",
        }
    }
}

impl NativePowerSourceState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AcPower => "ac",
            Self::BatteryPower => "battery",
            Self::Offline => "offline",
        }
    }
}

impl NativeThermalState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nominal => "nominal",
            Self::Fair => "fair",
            Self::Serious => "serious",
            Self::Critical => "critical",
        }
    }
}

pub fn copy_host_pressure() -> NativeHostPressure {
    let Some(process_info) = process_info() else {
        return NativeHostPressure {
            thermal_status: NativeHostSignalStatus::Unsupported,
            thermal_state: None,
            thermal_reason: Some("NSProcessInfo processInfo is unavailable".to_string()),
            low_power_status: NativeHostSignalStatus::Unsupported,
            low_power_enabled: None,
            low_power_reason: Some("NSProcessInfo processInfo is unavailable".to_string()),
            power_source_status: NativeHostSignalStatus::Unsupported,
            power_source_state: None,
            power_source_reason: Some("NSProcessInfo processInfo is unavailable".to_string()),
            user_activity_status: NativeHostSignalStatus::Unsupported,
            user_activity_state: None,
            user_activity_idle_millis: None,
            user_activity_reason: Some("NSProcessInfo processInfo is unavailable".to_string()),
        };
    };

    let (thermal_status, thermal_state, thermal_reason) = read_thermal_state(process_info);
    let (low_power_status, low_power_enabled, low_power_reason) = read_low_power_mode(process_info);
    let (power_source_status, power_source_state, power_source_reason) = read_power_source_state();
    let (
        user_activity_status,
        user_activity_state,
        user_activity_idle_millis,
        user_activity_reason,
    ) = read_user_activity();

    NativeHostPressure {
        thermal_status,
        thermal_state,
        thermal_reason,
        low_power_status,
        low_power_enabled,
        low_power_reason,
        power_source_status,
        power_source_state,
        power_source_reason,
        user_activity_status,
        user_activity_state,
        user_activity_idle_millis,
        user_activity_reason,
    }
}

fn process_info() -> Option<*mut Object> {
    let class = Class::get("NSProcessInfo")?;
    let selector = Sel::register("processInfo");
    let send: unsafe extern "C" fn(&Class, Sel) -> *mut Object =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    let object = unsafe { send(class, selector) };
    (!object.is_null()).then_some(object)
}

fn read_thermal_state(
    process_info: *mut Object,
) -> (
    NativeHostSignalStatus,
    Option<NativeThermalState>,
    Option<String>,
) {
    let selector = Sel::register("thermalState");
    if !object_responds_to_selector(process_info, selector) {
        return (
            NativeHostSignalStatus::Unsupported,
            None,
            Some("NSProcessInfo thermalState is unavailable".to_string()),
        );
    }
    let send: unsafe extern "C" fn(*mut Object, Sel) -> isize =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    match unsafe { send(process_info, selector) } {
        0 => (
            NativeHostSignalStatus::Available,
            Some(NativeThermalState::Nominal),
            None,
        ),
        1 => (
            NativeHostSignalStatus::Available,
            Some(NativeThermalState::Fair),
            None,
        ),
        2 => (
            NativeHostSignalStatus::Available,
            Some(NativeThermalState::Serious),
            None,
        ),
        3 => (
            NativeHostSignalStatus::Available,
            Some(NativeThermalState::Critical),
            None,
        ),
        value => (
            NativeHostSignalStatus::Unavailable,
            None,
            Some(format!(
                "NSProcessInfo returned unknown thermalState {value}"
            )),
        ),
    }
}

fn read_low_power_mode(
    process_info: *mut Object,
) -> (NativeHostSignalStatus, Option<bool>, Option<String>) {
    let selector = Sel::register("isLowPowerModeEnabled");
    if !object_responds_to_selector(process_info, selector) {
        return (
            NativeHostSignalStatus::Unsupported,
            None,
            Some("NSProcessInfo isLowPowerModeEnabled is unavailable".to_string()),
        );
    }
    let send: unsafe extern "C" fn(*mut Object, Sel) -> i8 =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    (
        NativeHostSignalStatus::Available,
        Some(unsafe { send(process_info, selector) != 0 }),
        None,
    )
}

fn read_power_source_state() -> (
    NativeHostSignalStatus,
    Option<NativePowerSourceState>,
    Option<String>,
) {
    let snapshot = unsafe { IOPSCopyPowerSourcesInfo() };
    if snapshot.is_null() {
        return (
            NativeHostSignalStatus::Unavailable,
            None,
            Some("IOKit did not return a power-source snapshot".to_string()),
        );
    }

    let sources = unsafe { IOPSCopyPowerSourcesList(snapshot) };
    if sources.is_null() {
        unsafe { CFRelease(snapshot) };
        return (
            NativeHostSignalStatus::Unavailable,
            None,
            Some("IOKit did not return a power-source list".to_string()),
        );
    }

    let state = read_power_source_state_from_list(snapshot, sources);
    unsafe {
        CFRelease(sources as CFTypeRef);
        CFRelease(snapshot);
    }
    state
}

fn read_power_source_state_from_list(
    snapshot: CFTypeRef,
    sources: CFArrayRef,
) -> (
    NativeHostSignalStatus,
    Option<NativePowerSourceState>,
    Option<String>,
) {
    let count = unsafe { CFArrayGetCount(sources) };
    if count == 0 {
        return (
            NativeHostSignalStatus::Available,
            Some(NativePowerSourceState::AcPower),
            None,
        );
    }

    let key = CFString::new("Power Source State");
    let mut saw_ac = false;
    let mut saw_offline = false;
    let mut saw_unknown = false;
    for index in 0..count {
        let source = unsafe { CFArrayGetValueAtIndex(sources, index) as CFTypeRef };
        if source.is_null() {
            saw_unknown = true;
            continue;
        }
        let description = unsafe { IOPSGetPowerSourceDescription(snapshot, source) };
        if description.is_null() {
            saw_unknown = true;
            continue;
        }
        let value = unsafe { CFDictionaryGetValue(description, key.as_CFTypeRef()) as CFTypeRef };
        match power_source_state_from_value(value) {
            Some(NativePowerSourceState::BatteryPower) => {
                return (
                    NativeHostSignalStatus::Available,
                    Some(NativePowerSourceState::BatteryPower),
                    None,
                );
            }
            Some(NativePowerSourceState::AcPower) => saw_ac = true,
            Some(NativePowerSourceState::Offline) => saw_offline = true,
            None => saw_unknown = true,
        }
    }

    if saw_ac {
        return (
            NativeHostSignalStatus::Available,
            Some(NativePowerSourceState::AcPower),
            None,
        );
    }
    if saw_offline {
        return (
            NativeHostSignalStatus::Unavailable,
            Some(NativePowerSourceState::Offline),
            Some("IOKit reports only offline power sources".to_string()),
        );
    }
    if saw_unknown {
        return (
            NativeHostSignalStatus::Unavailable,
            None,
            Some("IOKit power-source state was missing or unknown".to_string()),
        );
    }
    (
        NativeHostSignalStatus::Unavailable,
        None,
        Some("IOKit power-source list had no readable descriptions".to_string()),
    )
}

fn power_source_state_from_value(value: CFTypeRef) -> Option<NativePowerSourceState> {
    if value.is_null() || unsafe { CFGetTypeID(value) } != unsafe { CFStringGetTypeID() } {
        return None;
    }
    let state = unsafe { CFString::wrap_under_get_rule(value as CFStringRef) }.to_string();
    match state.as_str() {
        "AC Power" => Some(NativePowerSourceState::AcPower),
        "Battery Power" => Some(NativePowerSourceState::BatteryPower),
        "Off Line" => Some(NativePowerSourceState::Offline),
        _ => None,
    }
}

fn read_user_activity() -> (
    NativeHostSignalStatus,
    Option<NativeUserActivityState>,
    Option<u64>,
    Option<String>,
) {
    let seconds = unsafe {
        CGEventSourceSecondsSinceLastEventType(
            CG_EVENT_SOURCE_STATE_COMBINED_SESSION_STATE,
            CG_ANY_INPUT_EVENT_TYPE,
        )
    };
    if !seconds.is_finite() || seconds < 0.0 {
        return (
            NativeHostSignalStatus::Unavailable,
            None,
            None,
            Some(format!(
                "CoreGraphics returned invalid idle duration {seconds}"
            )),
        );
    }

    let idle_millis = (seconds * 1000.0).round() as u64;
    let activity = if seconds <= ACTIVE_INPUT_WINDOW_SECONDS {
        NativeUserActivityState::Active
    } else {
        NativeUserActivityState::Idle
    };
    (
        NativeHostSignalStatus::Available,
        Some(activity),
        Some(idle_millis),
        None,
    )
}

fn object_responds_to_selector(object: *mut Object, selector: Sel) -> bool {
    if object.is_null() {
        return false;
    }
    let responds: unsafe extern "C" fn(*mut Object, Sel, Sel) -> i8 =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { responds(object, Sel::register("respondsToSelector:"), selector) != 0 }
}
