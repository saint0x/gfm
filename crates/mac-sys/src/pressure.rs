use objc::runtime::{Class, Object, Sel};

#[link(name = "Foundation", kind = "framework")]
extern "C" {}

#[link(name = "objc")]
extern "C" {
    fn objc_msgSend();
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeHostPressure {
    pub thermal_status: NativeHostSignalStatus,
    pub thermal_state: Option<NativeThermalState>,
    pub thermal_reason: Option<String>,
    pub low_power_status: NativeHostSignalStatus,
    pub low_power_enabled: Option<bool>,
    pub low_power_reason: Option<String>,
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
        };
    };

    let (thermal_status, thermal_state, thermal_reason) = read_thermal_state(process_info);
    let (low_power_status, low_power_enabled, low_power_reason) = read_low_power_mode(process_info);

    NativeHostPressure {
        thermal_status,
        thermal_state,
        thermal_reason,
        low_power_status,
        low_power_enabled,
        low_power_reason,
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

fn object_responds_to_selector(object: *mut Object, selector: Sel) -> bool {
    if object.is_null() {
        return false;
    }
    let responds: unsafe extern "C" fn(*mut Object, Sel, Sel) -> i8 =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { responds(object, Sel::register("respondsToSelector:"), selector) != 0 }
}
