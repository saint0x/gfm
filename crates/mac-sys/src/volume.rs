use core_foundation::base::{kCFAllocatorDefault, CFType, TCFType};
use core_foundation::boolean::{CFBoolean, CFBooleanRef};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation::url::CFURL;
use core_foundation_sys::array::CFArrayRef;
use core_foundation_sys::base::{CFAllocatorRef, CFGetTypeID, CFRelease, CFTypeRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValueIfPresent, CFDictionaryRef};
use core_foundation_sys::runloop::{
    kCFRunLoopDefaultMode, CFRunLoopGetCurrent, CFRunLoopRef, CFRunLoopRunInMode, CFRunLoopStop,
    CFRunLoopWakeUp,
};
use core_foundation_sys::string::CFStringRef;
use core_foundation_sys::url::CFURLRef;
use core_foundation_sys::uuid::{CFUUIDCreateString, CFUUIDRef};
use libc::{c_void, statfs, MNT_LOCAL, MNT_NOWAIT, MNT_RDONLY};
use objc::runtime::{Object, Sel, BOOL};
use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::mpsc::{self, Receiver};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};

type DASessionRef = *const c_void;
type DADiskRef = *const c_void;
type DADissenterRef = *const c_void;
type DADiskAppearedCallback = Option<unsafe extern "C" fn(DADiskRef, *mut c_void)>;
type DADiskDisappearedCallback = Option<unsafe extern "C" fn(DADiskRef, *mut c_void)>;
type DADiskDescriptionChangedCallback =
    Option<unsafe extern "C" fn(DADiskRef, CFArrayRef, *mut c_void)>;
type DADiskEjectCallback = Option<unsafe extern "C" fn(DADiskRef, DADissenterRef, *mut c_void)>;
type DADiskMountCallback = Option<unsafe extern "C" fn(DADiskRef, DADissenterRef, *mut c_void)>;
type DADiskUnmountCallback = Option<unsafe extern "C" fn(DADiskRef, DADissenterRef, *mut c_void)>;

const DA_RETURN_BUSY: u32 = 0xF8DA0002;
const DA_RETURN_BAD_ARGUMENT: u32 = 0xF8DA0003;
const DA_RETURN_EXCLUSIVE_ACCESS: u32 = 0xF8DA0004;
const DA_RETURN_NO_RESOURCES: u32 = 0xF8DA0005;
const DA_RETURN_NOT_FOUND: u32 = 0xF8DA0006;
const DA_RETURN_NOT_MOUNTED: u32 = 0xF8DA0007;
const DA_RETURN_NOT_PERMITTED: u32 = 0xF8DA0008;
const DA_RETURN_NOT_PRIVILEGED: u32 = 0xF8DA0009;
const DA_RETURN_NOT_READY: u32 = 0xF8DA000A;
const DA_RETURN_NOT_WRITABLE: u32 = 0xF8DA000B;
const DA_RETURN_UNSUPPORTED: u32 = 0xF8DA000C;
const VOLUME_OPERATION_CALLBACK_TIMEOUT_SECONDS: f64 = 0.5;
const VOLUME_OPERATION_CALLBACK_TIMEOUT_MILLIS: u64 =
    (VOLUME_OPERATION_CALLBACK_TIMEOUT_SECONDS * 1000.0) as u64;

#[link(name = "DiskArbitration", kind = "framework")]
extern "C" {
    fn DASessionCreate(allocator: CFAllocatorRef) -> DASessionRef;
    fn DADiskCreateFromVolumePath(
        allocator: CFAllocatorRef,
        session: DASessionRef,
        path: CFURLRef,
    ) -> DADiskRef;
    fn DADiskCreateFromBSDName(
        allocator: CFAllocatorRef,
        session: DASessionRef,
        name: *const libc::c_char,
    ) -> DADiskRef;
    fn DADiskCopyDescription(disk: DADiskRef) -> CFDictionaryRef;
    fn DADiskCopyWholeDisk(disk: DADiskRef) -> DADiskRef;
    fn DADiskEject(
        disk: DADiskRef,
        options: u32,
        callback: DADiskEjectCallback,
        context: *mut c_void,
    );
    fn DADiskMount(
        disk: DADiskRef,
        path: CFURLRef,
        options: u32,
        callback: DADiskMountCallback,
        context: *mut c_void,
    );
    fn DADiskUnmount(
        disk: DADiskRef,
        options: u32,
        callback: DADiskUnmountCallback,
        context: *mut c_void,
    );
    fn DADissenterGetStatus(dissenter: DADissenterRef) -> i32;
    fn DADissenterGetStatusString(dissenter: DADissenterRef) -> CFStringRef;
    fn DASessionScheduleWithRunLoop(
        session: DASessionRef,
        run_loop: CFRunLoopRef,
        run_loop_mode: CFStringRef,
    );
    fn DASessionUnscheduleFromRunLoop(
        session: DASessionRef,
        run_loop: CFRunLoopRef,
        run_loop_mode: CFStringRef,
    );
    fn DARegisterDiskAppearedCallback(
        session: DASessionRef,
        match_description: CFDictionaryRef,
        callback: DADiskAppearedCallback,
        context: *mut c_void,
    );
    fn DARegisterDiskDescriptionChangedCallback(
        session: DASessionRef,
        match_description: CFDictionaryRef,
        watch: CFArrayRef,
        callback: DADiskDescriptionChangedCallback,
        context: *mut c_void,
    );
    fn DARegisterDiskDisappearedCallback(
        session: DASessionRef,
        match_description: CFDictionaryRef,
        callback: DADiskDisappearedCallback,
        context: *mut c_void,
    );

    static kDADiskDescriptionDeviceInternalKey: CFStringRef;
    static kDADiskDescriptionDeviceModelKey: CFStringRef;
    static kDADiskDescriptionDevicePathKey: CFStringRef;
    static kDADiskDescriptionDeviceProtocolKey: CFStringRef;
    static kDADiskDescriptionDeviceVendorKey: CFStringRef;
    static kDADiskDescriptionMediaBlockSizeKey: CFStringRef;
    static kDADiskDescriptionMediaBSDMajorKey: CFStringRef;
    static kDADiskDescriptionMediaBSDMinorKey: CFStringRef;
    static kDADiskDescriptionMediaBSDNameKey: CFStringRef;
    static kDADiskDescriptionMediaBSDUnitKey: CFStringRef;
    static kDADiskDescriptionMediaContentKey: CFStringRef;
    static kDADiskDescriptionMediaEncryptedKey: CFStringRef;
    static kDADiskDescriptionMediaEjectableKey: CFStringRef;
    static kDADiskDescriptionMediaKindKey: CFStringRef;
    static kDADiskDescriptionMediaLeafKey: CFStringRef;
    static kDADiskDescriptionMediaNameKey: CFStringRef;
    static kDADiskDescriptionMediaPathKey: CFStringRef;
    static kDADiskDescriptionMediaRemovableKey: CFStringRef;
    static kDADiskDescriptionMediaSizeKey: CFStringRef;
    static kDADiskDescriptionMediaTypeKey: CFStringRef;
    static kDADiskDescriptionMediaUUIDKey: CFStringRef;
    static kDADiskDescriptionMediaWholeKey: CFStringRef;
    static kDADiskDescriptionMediaWritableKey: CFStringRef;
    static kDADiskDescriptionVolumeKindKey: CFStringRef;
    static kDADiskDescriptionVolumeMountableKey: CFStringRef;
    static kDADiskDescriptionVolumeNameKey: CFStringRef;
    static kDADiskDescriptionVolumeNetworkKey: CFStringRef;
    static kDADiskDescriptionVolumePathKey: CFStringRef;
    static kDADiskDescriptionVolumeTypeKey: CFStringRef;
    static kDADiskDescriptionVolumeUUIDKey: CFStringRef;
}

#[link(name = "Foundation", kind = "framework")]
extern "C" {
    static NSURLVolumeIsAutomountedKey: CFStringRef;
    static NSURLVolumeIsBrowsableKey: CFStringRef;
    static NSURLVolumeIsEjectableKey: CFStringRef;
    static NSURLVolumeIsInternalKey: CFStringRef;
    static NSURLVolumeIsLocalKey: CFStringRef;
    static NSURLVolumeIsReadOnlyKey: CFStringRef;
    static NSURLVolumeIsRemovableKey: CFStringRef;
    static NSURLVolumeURLForRemountingKey: CFStringRef;
    static NSURLVolumeUUIDStringKey: CFStringRef;
    static NSURLVolumeSupportsCasePreservedNamesKey: CFStringRef;
    static NSURLVolumeSupportsCaseSensitiveNamesKey: CFStringRef;

    fn CFURLCopyResourcePropertyForKey(
        url: CFURLRef,
        key: CFStringRef,
        property_value_type_ref_ptr: *mut CFTypeRef,
        error: *mut core_foundation_sys::error::CFErrorRef,
    ) -> core_foundation_sys::base::Boolean;
}

#[link(name = "objc")]
extern "C" {
    fn objc_msgSend();
    fn sel_registerName(name: *const libc::c_char) -> Sel;
}

extern "C" {
    fn CFBooleanGetTypeID() -> core_foundation_sys::base::CFTypeID;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeVolumeDescription {
    pub status: NativeVolumeStatus,
    pub volume_name: Option<String>,
    pub volume_kind: Option<String>,
    pub volume_mountable: Option<bool>,
    pub volume_type: Option<String>,
    pub volume_uuid: Option<String>,
    pub volume_path: Option<PathBuf>,
    pub volume_network: Option<bool>,
    pub media_bsd_name: Option<String>,
    pub media_bsd_major: Option<u64>,
    pub media_bsd_minor: Option<u64>,
    pub media_bsd_unit: Option<u64>,
    pub media_content: Option<String>,
    pub media_kind: Option<String>,
    pub media_leaf: Option<bool>,
    pub media_name: Option<String>,
    pub media_path: Option<String>,
    pub media_removable: Option<bool>,
    pub media_ejectable: Option<bool>,
    pub media_writable: Option<bool>,
    pub media_type: Option<String>,
    pub media_uuid: Option<String>,
    pub whole_disk_media_uuid: Option<String>,
    pub media_whole: Option<bool>,
    pub media_encrypted: Option<bool>,
    pub media_block_size_bytes: Option<u64>,
    pub media_size_bytes: Option<u64>,
    pub device_internal: Option<bool>,
    pub device_model: Option<String>,
    pub device_path: Option<String>,
    pub device_protocol: Option<String>,
    pub device_vendor: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeVolumeResourceValues {
    pub status: NativeVolumeStatus,
    pub is_automounted: Option<bool>,
    pub is_browsable: Option<bool>,
    pub is_ejectable: Option<bool>,
    pub is_internal: Option<bool>,
    pub is_local: Option<bool>,
    pub is_read_only: Option<bool>,
    pub is_reachable: Option<bool>,
    pub is_removable: Option<bool>,
    pub remount_url: Option<String>,
    pub supports_case_preserved_names: Option<bool>,
    pub supports_case_sensitive_names: Option<bool>,
    pub volume_uuid: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeVolumeMountTableEntry {
    pub status: NativeVolumeStatus,
    pub mount_point: Option<PathBuf>,
    pub mounted_from: Option<String>,
    pub filesystem_type: Option<String>,
    pub flags: Option<u32>,
    pub is_read_only: Option<bool>,
    pub is_local: Option<bool>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeVolumeMountTable {
    pub status: NativeVolumeStatus,
    pub entries: Vec<NativeVolumeMountTableEntry>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeVolumeEventKind {
    Appeared,
    DescriptionChanged,
    Disappeared,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeVolumeEvent {
    pub kind: NativeVolumeEventKind,
    pub description: NativeVolumeDescription,
}

pub struct NativeVolumeEventStream {
    receiver: Receiver<NativeVolumeEvent>,
    run_loop: Option<usize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

struct NativeVolumeEventContext {
    sender: mpsc::Sender<NativeVolumeEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeVolumeStatus {
    Available,
    Missing,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeVolumeOperation {
    Eject,
    Mount,
    Unmount,
}

impl NativeVolumeOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Eject => "eject",
            Self::Mount => "mount",
            Self::Unmount => "unmount",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeVolumeOperationStatus {
    Succeeded,
    Submitted,
    Busy,
    BadArgument,
    ExclusiveAccess,
    NoResources,
    NotFound,
    NotMounted,
    NotPermitted,
    NotPrivileged,
    NotReady,
    NotWritable,
    Unsupported,
    Failed,
    Missing,
    Unavailable,
}

impl NativeVolumeOperationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Submitted => "submitted",
            Self::Busy => "busy",
            Self::BadArgument => "bad-argument",
            Self::ExclusiveAccess => "exclusive-access",
            Self::NoResources => "no-resources",
            Self::NotFound => "not-found",
            Self::NotMounted => "not-mounted",
            Self::NotPermitted => "not-permitted",
            Self::NotPrivileged => "not-privileged",
            Self::NotReady => "not-ready",
            Self::NotWritable => "not-writable",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeVolumeOperationResult {
    pub operation: NativeVolumeOperation,
    pub status: NativeVolumeOperationStatus,
    pub dissenter_status: Option<u32>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeVolumeEventShutdown {
    pub attached_before_shutdown: bool,
    pub stop_requested: bool,
    pub thread_joined: bool,
}

struct NativeVolumeOperationContext {
    operation: NativeVolumeOperation,
    run_loop: CFRunLoopRef,
    sender: mpsc::Sender<NativeVolumeOperationResult>,
}

impl NativeVolumeEventStream {
    pub fn start() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let session = unsafe { DASessionCreate(kCFAllocatorDefault) };
            if session.is_null() {
                let _ = event_tx.send(NativeVolumeEvent {
                    kind: NativeVolumeEventKind::Unavailable,
                    description: unavailable("DiskArbitration did not create an event session"),
                });
                let _ = ready_tx.send(None);
                return;
            }

            let context = Box::into_raw(Box::new(NativeVolumeEventContext { sender: event_tx }));
            unsafe {
                DARegisterDiskAppearedCallback(
                    session,
                    ptr::null(),
                    Some(disk_appeared_callback),
                    context.cast(),
                );
                DARegisterDiskDescriptionChangedCallback(
                    session,
                    ptr::null(),
                    ptr::null(),
                    Some(disk_description_changed_callback),
                    context.cast(),
                );
                DARegisterDiskDisappearedCallback(
                    session,
                    ptr::null(),
                    Some(disk_disappeared_callback),
                    context.cast(),
                );
            }

            let run_loop = unsafe { CFRunLoopGetCurrent() };
            unsafe {
                DASessionScheduleWithRunLoop(session, run_loop, kCFRunLoopDefaultMode);
            }
            let _ = ready_tx.send(Some(run_loop as usize));
            unsafe {
                while !thread_stop.load(Ordering::Acquire) {
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.25, 1);
                }
                DASessionUnscheduleFromRunLoop(session, run_loop, kCFRunLoopDefaultMode);
                CFRelease(session as CFTypeRef);
                drop(Box::from_raw(context));
            }
        });

        let run_loop = ready_rx.recv().ok().flatten();
        Self {
            receiver: event_rx,
            run_loop,
            stop,
            thread: Some(thread),
        }
    }

    pub fn is_attached(&self) -> bool {
        self.run_loop.is_some()
    }

    pub fn try_recv(&self) -> Option<NativeVolumeEvent> {
        self.receiver.try_recv().ok()
    }

    pub fn shutdown(mut self) -> NativeVolumeEventShutdown {
        self.shutdown_inner()
    }

    fn shutdown_inner(&mut self) -> NativeVolumeEventShutdown {
        let attached_before_shutdown = self.run_loop.is_some();
        self.stop.store(true, Ordering::Release);
        if let Some(run_loop) = self.run_loop.take() {
            unsafe {
                let run_loop = run_loop as CFRunLoopRef;
                CFRunLoopStop(run_loop);
                CFRunLoopWakeUp(run_loop);
            }
        }
        let thread_joined = self
            .thread
            .take()
            .map(|thread| thread.join().is_ok())
            .unwrap_or(true);
        NativeVolumeEventShutdown {
            attached_before_shutdown,
            stop_requested: true,
            thread_joined,
        }
    }
}

impl Drop for NativeVolumeEventStream {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

impl NativeVolumeStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
        }
    }
}

pub fn copy_volume_description_for_path(path: &Path) -> NativeVolumeDescription {
    if !path.exists() {
        return missing(format!("volume path does not exist: {}", path.display()));
    }
    let Some(url) = CFURL::from_path(path, true) else {
        return unavailable(format!("invalid volume path URL: {}", path.display()));
    };

    let session = unsafe { DASessionCreate(kCFAllocatorDefault) };
    if session.is_null() {
        return unavailable("DiskArbitration did not create a session");
    }

    let disk = unsafe {
        DADiskCreateFromVolumePath(kCFAllocatorDefault, session, url.as_concrete_TypeRef())
    };
    if disk.is_null() {
        unsafe {
            CFRelease(session as CFTypeRef);
        }
        return unavailable(format!(
            "DiskArbitration did not return a disk for {}",
            path.display()
        ));
    }

    let description = volume_description_from_disk(disk);
    unsafe {
        CFRelease(disk as CFTypeRef);
        CFRelease(session as CFTypeRef);
    }
    description
}

fn volume_description_from_disk(disk: DADiskRef) -> NativeVolumeDescription {
    if disk.is_null() {
        return unavailable("DiskArbitration callback received a null disk");
    }
    let description = unsafe { DADiskCopyDescription(disk) };
    if description.is_null() {
        return unavailable("DiskArbitration did not return a disk description");
    }
    let description = unsafe {
        CFDictionary::<*const c_void, *const c_void>::wrap_under_create_rule(description)
    };
    NativeVolumeDescription {
        status: NativeVolumeStatus::Available,
        volume_name: string_value(&description, unsafe { kDADiskDescriptionVolumeNameKey }),
        volume_kind: string_value(&description, unsafe { kDADiskDescriptionVolumeKindKey }),
        volume_mountable: bool_value(&description, unsafe {
            kDADiskDescriptionVolumeMountableKey
        }),
        volume_type: string_value(&description, unsafe { kDADiskDescriptionVolumeTypeKey }),
        volume_uuid: uuid_value(&description, unsafe { kDADiskDescriptionVolumeUUIDKey }),
        volume_path: url_value(&description, unsafe { kDADiskDescriptionVolumePathKey }),
        volume_network: bool_value(&description, unsafe { kDADiskDescriptionVolumeNetworkKey }),
        media_bsd_name: string_value(&description, unsafe { kDADiskDescriptionMediaBSDNameKey }),
        media_bsd_major: u64_value(&description, unsafe { kDADiskDescriptionMediaBSDMajorKey }),
        media_bsd_minor: u64_value(&description, unsafe { kDADiskDescriptionMediaBSDMinorKey }),
        media_bsd_unit: u64_value(&description, unsafe { kDADiskDescriptionMediaBSDUnitKey }),
        media_content: string_value(&description, unsafe { kDADiskDescriptionMediaContentKey }),
        media_kind: string_value(&description, unsafe { kDADiskDescriptionMediaKindKey }),
        media_leaf: bool_value(&description, unsafe { kDADiskDescriptionMediaLeafKey }),
        media_name: string_value(&description, unsafe { kDADiskDescriptionMediaNameKey }),
        media_path: string_value(&description, unsafe { kDADiskDescriptionMediaPathKey }),
        media_removable: bool_value(&description, unsafe { kDADiskDescriptionMediaRemovableKey }),
        media_ejectable: bool_value(&description, unsafe { kDADiskDescriptionMediaEjectableKey }),
        media_writable: bool_value(&description, unsafe { kDADiskDescriptionMediaWritableKey }),
        media_type: string_value(&description, unsafe { kDADiskDescriptionMediaTypeKey }),
        media_uuid: uuid_value(&description, unsafe { kDADiskDescriptionMediaUUIDKey }),
        whole_disk_media_uuid: whole_disk_media_uuid(disk),
        media_whole: bool_value(&description, unsafe { kDADiskDescriptionMediaWholeKey }),
        media_encrypted: bool_value(&description, unsafe { kDADiskDescriptionMediaEncryptedKey }),
        media_block_size_bytes: u64_value(&description, unsafe {
            kDADiskDescriptionMediaBlockSizeKey
        }),
        media_size_bytes: u64_value(&description, unsafe { kDADiskDescriptionMediaSizeKey }),
        device_internal: bool_value(&description, unsafe { kDADiskDescriptionDeviceInternalKey }),
        device_model: string_value(&description, unsafe { kDADiskDescriptionDeviceModelKey }),
        device_path: string_value(&description, unsafe { kDADiskDescriptionDevicePathKey }),
        device_protocol: string_value(&description, unsafe { kDADiskDescriptionDeviceProtocolKey }),
        device_vendor: string_value(&description, unsafe { kDADiskDescriptionDeviceVendorKey }),
        reason: None,
    }
}

unsafe extern "C" fn disk_appeared_callback(disk: DADiskRef, context: *mut c_void) {
    send_volume_event(context, NativeVolumeEventKind::Appeared, disk);
}

unsafe extern "C" fn disk_description_changed_callback(
    disk: DADiskRef,
    _keys: CFArrayRef,
    context: *mut c_void,
) {
    send_volume_event(context, NativeVolumeEventKind::DescriptionChanged, disk);
}

unsafe extern "C" fn disk_disappeared_callback(disk: DADiskRef, context: *mut c_void) {
    send_volume_event(context, NativeVolumeEventKind::Disappeared, disk);
}

fn send_volume_event(context: *mut c_void, kind: NativeVolumeEventKind, disk: DADiskRef) {
    if context.is_null() {
        return;
    }
    let context = unsafe { &*(context as *const NativeVolumeEventContext) };
    let _ = context.sender.send(NativeVolumeEvent {
        kind,
        description: volume_description_from_disk(disk),
    });
}

pub fn submit_volume_operation(
    path: &Path,
    operation: NativeVolumeOperation,
) -> NativeVolumeOperationResult {
    let Some((session, disk)) = create_disk_for_volume_path(path) else {
        return NativeVolumeOperationResult {
            operation,
            status: if path.exists() {
                NativeVolumeOperationStatus::Unavailable
            } else {
                NativeVolumeOperationStatus::Missing
            },
            dissenter_status: None,
            reason: Some(if path.exists() {
                format!(
                    "DiskArbitration did not return a disk for {}",
                    path.display()
                )
            } else {
                format!("volume path does not exist: {}", path.display())
            }),
        };
    };

    let run_loop = unsafe { CFRunLoopGetCurrent() };
    let (tx, rx) = mpsc::channel();
    let context = Box::into_raw(Box::new(NativeVolumeOperationContext {
        operation,
        run_loop,
        sender: tx,
    }));
    unsafe {
        DASessionScheduleWithRunLoop(session, run_loop, kCFRunLoopDefaultMode);
    }
    match operation {
        NativeVolumeOperation::Eject => unsafe {
            DADiskEject(disk, 0, Some(volume_operation_callback), context.cast());
        },
        NativeVolumeOperation::Mount => unsafe {
            DADiskMount(
                disk,
                ptr::null(),
                0,
                Some(volume_operation_callback),
                context.cast(),
            );
        },
        NativeVolumeOperation::Unmount => unsafe {
            DADiskUnmount(disk, 0, Some(volume_operation_callback), context.cast());
        },
    }

    unsafe {
        CFRunLoopRunInMode(
            kCFRunLoopDefaultMode,
            VOLUME_OPERATION_CALLBACK_TIMEOUT_SECONDS,
            0,
        );
        DASessionUnscheduleFromRunLoop(session, run_loop, kCFRunLoopDefaultMode);
    }
    let result = if let Ok(result) = rx.try_recv() {
        unsafe {
            drop(Box::from_raw(context));
        }
        result
    } else {
        NativeVolumeOperationResult {
            operation,
            status: NativeVolumeOperationStatus::Submitted,
            dissenter_status: None,
            reason: Some(volume_operation_submitted_reason()),
        }
    };

    unsafe {
        CFRelease(disk as CFTypeRef);
        CFRelease(session as CFTypeRef);
    }

    result
}

pub fn submit_volume_mount_by_bsd_name(bsd_name: &str) -> NativeVolumeOperationResult {
    let operation = NativeVolumeOperation::Mount;
    if !valid_bsd_disk_name(bsd_name) {
        return NativeVolumeOperationResult {
            operation,
            status: NativeVolumeOperationStatus::Unsupported,
            dissenter_status: None,
            reason: Some("diskarbitration-mount-requires-bsd-name".to_string()),
        };
    }

    let session = unsafe { DASessionCreate(kCFAllocatorDefault) };
    if session.is_null() {
        return NativeVolumeOperationResult {
            operation,
            status: NativeVolumeOperationStatus::Unavailable,
            dissenter_status: None,
            reason: Some("DiskArbitration did not create an operation session".to_string()),
        };
    }

    let bsd_name = CString::new(bsd_name).expect("BSD name was checked for interior NUL");
    let disk = unsafe { DADiskCreateFromBSDName(kCFAllocatorDefault, session, bsd_name.as_ptr()) };
    if disk.is_null() {
        unsafe {
            CFRelease(session as CFTypeRef);
        }
        return NativeVolumeOperationResult {
            operation,
            status: NativeVolumeOperationStatus::Missing,
            dissenter_status: None,
            reason: Some(format!(
                "DiskArbitration did not return a disk for {}",
                bsd_name.to_string_lossy()
            )),
        };
    }

    let run_loop = unsafe { CFRunLoopGetCurrent() };
    let (tx, rx) = mpsc::channel();
    let context = Box::into_raw(Box::new(NativeVolumeOperationContext {
        operation,
        run_loop,
        sender: tx,
    }));
    unsafe {
        DASessionScheduleWithRunLoop(session, run_loop, kCFRunLoopDefaultMode);
        DADiskMount(
            disk,
            ptr::null(),
            0,
            Some(volume_operation_callback),
            context.cast(),
        );
        CFRunLoopRunInMode(
            kCFRunLoopDefaultMode,
            VOLUME_OPERATION_CALLBACK_TIMEOUT_SECONDS,
            0,
        );
        DASessionUnscheduleFromRunLoop(session, run_loop, kCFRunLoopDefaultMode);
    }
    let result = if let Ok(result) = rx.try_recv() {
        unsafe {
            drop(Box::from_raw(context));
        }
        result
    } else {
        NativeVolumeOperationResult {
            operation,
            status: NativeVolumeOperationStatus::Submitted,
            dissenter_status: None,
            reason: Some(volume_operation_submitted_reason()),
        }
    };

    unsafe {
        CFRelease(disk as CFTypeRef);
        CFRelease(session as CFTypeRef);
    }

    result
}

fn valid_bsd_disk_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("disk") else {
        return false;
    };
    let bytes = rest.as_bytes();
    if bytes.is_empty() || bytes.contains(&0) || bytes.contains(&b'/') {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == 0 {
        return false;
    }
    if index == bytes.len() {
        return true;
    }
    if bytes[index] != b's' {
        return false;
    }
    index += 1;
    let slice_start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    index == bytes.len() && index > slice_start
}

unsafe extern "C" fn volume_operation_callback(
    _disk: DADiskRef,
    dissenter: DADissenterRef,
    context: *mut c_void,
) {
    if context.is_null() {
        return;
    }
    let context = &*(context as *const NativeVolumeOperationContext);
    let (status, dissenter_status, reason) = if dissenter.is_null() {
        (
            NativeVolumeOperationStatus::Succeeded,
            None,
            Some("diskarbitration-operation-succeeded".to_string()),
        )
    } else {
        let code = DADissenterGetStatus(dissenter) as u32;
        (
            native_operation_status_for_dissenter(code),
            Some(code),
            Some(dissenter_reason(dissenter, code)),
        )
    };
    let _ = context.sender.send(NativeVolumeOperationResult {
        operation: context.operation,
        status,
        dissenter_status,
        reason,
    });
    CFRunLoopStop(context.run_loop);
    CFRunLoopWakeUp(context.run_loop);
}

fn native_operation_status_for_dissenter(code: u32) -> NativeVolumeOperationStatus {
    match code {
        DA_RETURN_BUSY => NativeVolumeOperationStatus::Busy,
        DA_RETURN_BAD_ARGUMENT => NativeVolumeOperationStatus::BadArgument,
        DA_RETURN_EXCLUSIVE_ACCESS => NativeVolumeOperationStatus::ExclusiveAccess,
        DA_RETURN_NO_RESOURCES => NativeVolumeOperationStatus::NoResources,
        DA_RETURN_NOT_FOUND => NativeVolumeOperationStatus::NotFound,
        DA_RETURN_NOT_MOUNTED => NativeVolumeOperationStatus::NotMounted,
        DA_RETURN_NOT_PERMITTED => NativeVolumeOperationStatus::NotPermitted,
        DA_RETURN_NOT_PRIVILEGED => NativeVolumeOperationStatus::NotPrivileged,
        DA_RETURN_NOT_READY => NativeVolumeOperationStatus::NotReady,
        DA_RETURN_NOT_WRITABLE => NativeVolumeOperationStatus::NotWritable,
        DA_RETURN_UNSUPPORTED => NativeVolumeOperationStatus::Unsupported,
        _ => NativeVolumeOperationStatus::Failed,
    }
}

fn volume_operation_submitted_reason() -> String {
    format!("submitted-to-diskarbitration-timeout-{VOLUME_OPERATION_CALLBACK_TIMEOUT_MILLIS}ms")
}

fn dissenter_reason(dissenter: DADissenterRef, code: u32) -> String {
    let status = unsafe { DADissenterGetStatusString(dissenter) };
    if status.is_null() {
        return format!("diskarbitration-dissenter-0x{code:08x}");
    }
    let status = unsafe { CFString::wrap_under_get_rule(status) }.to_string();
    if status.is_empty() {
        format!("diskarbitration-dissenter-0x{code:08x}")
    } else {
        format!("diskarbitration-dissenter-0x{code:08x}:{status}")
    }
}

pub fn copy_volume_resource_values(path: &Path) -> NativeVolumeResourceValues {
    if !path.exists() {
        return unavailable_resource_values(
            NativeVolumeStatus::Missing,
            format!("volume path does not exist: {}", path.display()),
        );
    }
    let Some(url) = CFURL::from_path(path, path.is_dir()) else {
        return unavailable_resource_values(
            NativeVolumeStatus::Unavailable,
            format!("invalid volume path URL: {}", path.display()),
        );
    };
    let url = url.as_concrete_TypeRef();

    NativeVolumeResourceValues {
        status: NativeVolumeStatus::Available,
        is_automounted: copy_resource_bool(url, unsafe { NSURLVolumeIsAutomountedKey }),
        is_browsable: copy_resource_bool(url, unsafe { NSURLVolumeIsBrowsableKey }),
        is_ejectable: copy_resource_bool(url, unsafe { NSURLVolumeIsEjectableKey }),
        is_internal: copy_resource_bool(url, unsafe { NSURLVolumeIsInternalKey }),
        is_local: copy_resource_bool(url, unsafe { NSURLVolumeIsLocalKey }),
        is_read_only: copy_resource_bool(url, unsafe { NSURLVolumeIsReadOnlyKey }),
        is_reachable: check_resource_is_reachable(url),
        is_removable: copy_resource_bool(url, unsafe { NSURLVolumeIsRemovableKey }),
        remount_url: copy_resource_url_string(url, unsafe { NSURLVolumeURLForRemountingKey }),
        supports_case_preserved_names: copy_resource_bool(url, unsafe {
            NSURLVolumeSupportsCasePreservedNamesKey
        }),
        supports_case_sensitive_names: copy_resource_bool(url, unsafe {
            NSURLVolumeSupportsCaseSensitiveNamesKey
        }),
        volume_uuid: copy_resource_string(url, unsafe { NSURLVolumeUUIDStringKey }),
        reason: None,
    }
}

pub fn copy_volume_mount_table_entry(path: &Path) -> NativeVolumeMountTableEntry {
    if !path.exists() {
        return unavailable_mount_table_entry(
            NativeVolumeStatus::Missing,
            format!("volume path does not exist: {}", path.display()),
        );
    }
    let display_path = path.display().to_string();
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return unavailable_mount_table_entry(
            NativeVolumeStatus::Unavailable,
            format!("volume path contains an interior NUL: {display_path}"),
        );
    };
    let mut info = std::mem::MaybeUninit::<statfs>::uninit();
    let copied = unsafe { libc::statfs(c_path.as_ptr(), info.as_mut_ptr()) };
    if copied != 0 {
        let error = std::io::Error::last_os_error();
        return unavailable_mount_table_entry(
            NativeVolumeStatus::Unavailable,
            format!("statfs failed for {display_path}: {error}"),
        );
    }
    native_mount_table_entry(unsafe { info.assume_init() })
}

pub fn copy_volume_mount_table() -> NativeVolumeMountTable {
    let mut mounts = ptr::null_mut::<statfs>();
    let count = unsafe { libc::getmntinfo(&mut mounts, MNT_NOWAIT) };
    if count <= 0 || mounts.is_null() {
        let error = std::io::Error::last_os_error();
        return NativeVolumeMountTable {
            status: NativeVolumeStatus::Unavailable,
            entries: Vec::new(),
            reason: Some(format!("getmntinfo failed: {error}")),
        };
    }

    let entries = unsafe { std::slice::from_raw_parts(mounts, count as usize) }
        .iter()
        .copied()
        .map(native_mount_table_entry)
        .collect();
    NativeVolumeMountTable {
        status: NativeVolumeStatus::Available,
        entries,
        reason: None,
    }
}

fn create_disk_for_volume_path(path: &Path) -> Option<(DASessionRef, DADiskRef)> {
    if !path.exists() {
        return None;
    }
    let url = CFURL::from_path(path, true)?;
    let session = unsafe { DASessionCreate(kCFAllocatorDefault) };
    if session.is_null() {
        return None;
    }
    let disk = unsafe {
        DADiskCreateFromVolumePath(kCFAllocatorDefault, session, url.as_concrete_TypeRef())
    };
    if disk.is_null() {
        unsafe {
            CFRelease(session as CFTypeRef);
        }
        return None;
    }
    Some((session, disk))
}

fn string_value(
    description: &CFDictionary<*const c_void, *const c_void>,
    key: CFStringRef,
) -> Option<String> {
    value_for_key(description, key)
        .map(|raw| unsafe { CFString::wrap_under_get_rule(raw as CFStringRef) })
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
}

fn bool_value(
    description: &CFDictionary<*const c_void, *const c_void>,
    key: CFStringRef,
) -> Option<bool> {
    value_for_key(description, key)
        .map(|raw| unsafe { CFBoolean::wrap_under_get_rule(raw as _) })
        .map(bool::from)
}

fn u64_value(
    description: &CFDictionary<*const c_void, *const c_void>,
    key: CFStringRef,
) -> Option<u64> {
    value_for_key(description, key)
        .and_then(|raw| unsafe { CFNumber::wrap_under_get_rule(raw as _) }.to_i64())
        .and_then(|value| u64::try_from(value).ok())
}

fn uuid_value(
    description: &CFDictionary<*const c_void, *const c_void>,
    key: CFStringRef,
) -> Option<String> {
    value_for_key(description, key)
        .and_then(|raw| {
            let value = unsafe { CFUUIDCreateString(kCFAllocatorDefault, raw as CFUUIDRef) };
            (!value.is_null()).then_some(value)
        })
        .map(|value| unsafe { CFString::wrap_under_create_rule(value) })
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
}

fn whole_disk_media_uuid(disk: DADiskRef) -> Option<String> {
    let whole_disk = unsafe { DADiskCopyWholeDisk(disk) };
    if whole_disk.is_null() {
        return None;
    }
    let description = unsafe { DADiskCopyDescription(whole_disk) };
    unsafe {
        CFRelease(whole_disk as CFTypeRef);
    }
    if description.is_null() {
        return None;
    }
    let description = unsafe {
        CFDictionary::<*const c_void, *const c_void>::wrap_under_create_rule(description)
    };
    uuid_value(&description, unsafe { kDADiskDescriptionMediaUUIDKey })
}

fn url_value(
    description: &CFDictionary<*const c_void, *const c_void>,
    key: CFStringRef,
) -> Option<PathBuf> {
    value_for_key(description, key)
        .map(|raw| unsafe { CFURL::wrap_under_get_rule(raw as CFURLRef) })
        .and_then(|url| url.to_path())
}

fn copy_resource_bool(url: CFURLRef, key: CFStringRef) -> Option<bool> {
    let value = copy_resource_value(url, key)?;
    if unsafe { CFGetTypeID(value.as_CFTypeRef()) } != unsafe { CFBooleanGetTypeID() } {
        return None;
    }
    let typed = unsafe { CFBoolean::wrap_under_get_rule(value.as_CFTypeRef() as CFBooleanRef) };
    Some(typed.into())
}

fn copy_resource_string(url: CFURLRef, key: CFStringRef) -> Option<String> {
    copy_resource_value(url, key)?
        .downcast::<CFString>()
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
}

fn copy_resource_url_string(url: CFURLRef, key: CFStringRef) -> Option<String> {
    copy_resource_value(url, key)?
        .downcast::<CFURL>()
        .map(|url| url.get_string().to_string())
        .filter(|value| !value.is_empty())
}

fn check_resource_is_reachable(url: CFURLRef) -> Option<bool> {
    if url.is_null() {
        return None;
    }
    let url = url as *mut Object;
    let mut error: *mut Object = ptr::null_mut();
    let selector = unsafe { sel_registerName(c"checkResourceIsReachableAndReturnError:".as_ptr()) };
    let send: unsafe extern "C" fn(*mut Object, Sel, *mut *mut Object) -> BOOL =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    Some(unsafe { send(url, selector, &mut error) })
}

fn copy_resource_value(url: CFURLRef, key: CFStringRef) -> Option<CFType> {
    let mut value: CFTypeRef = ptr::null();
    let copied = unsafe { CFURLCopyResourcePropertyForKey(url, key, &mut value, ptr::null_mut()) };
    if copied == 0 || value.is_null() {
        None
    } else {
        Some(unsafe { CFType::wrap_under_create_rule(value) })
    }
}

fn c_char_array_to_string(buffer: &[libc::c_char]) -> Option<String> {
    if buffer.first().copied().unwrap_or_default() == 0 {
        return None;
    }
    let value = unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    (!value.is_empty()).then_some(value)
}

fn native_mount_table_entry(info: statfs) -> NativeVolumeMountTableEntry {
    let flags = info.f_flags;
    NativeVolumeMountTableEntry {
        status: NativeVolumeStatus::Available,
        mount_point: c_char_array_to_string(&info.f_mntonname).map(PathBuf::from),
        mounted_from: c_char_array_to_string(&info.f_mntfromname),
        filesystem_type: c_char_array_to_string(&info.f_fstypename),
        flags: Some(flags),
        is_read_only: Some((flags & MNT_RDONLY as u32) != 0),
        is_local: Some((flags & MNT_LOCAL as u32) != 0),
        reason: None,
    }
}

fn value_for_key(
    description: &CFDictionary<*const c_void, *const c_void>,
    key: CFStringRef,
) -> Option<CFTypeRef> {
    let mut value = ptr::null();
    let present = unsafe {
        CFDictionaryGetValueIfPresent(
            description.as_concrete_TypeRef(),
            key as *const c_void,
            &mut value,
        )
    };
    (present != 0 && !value.is_null()).then_some(value as CFTypeRef)
}

fn missing(reason: impl Into<String>) -> NativeVolumeDescription {
    unavailable_with_status(NativeVolumeStatus::Missing, reason)
}

fn unavailable(reason: impl Into<String>) -> NativeVolumeDescription {
    unavailable_with_status(NativeVolumeStatus::Unavailable, reason)
}

fn unavailable_with_status(
    status: NativeVolumeStatus,
    reason: impl Into<String>,
) -> NativeVolumeDescription {
    NativeVolumeDescription {
        status,
        volume_name: None,
        volume_kind: None,
        volume_mountable: None,
        volume_type: None,
        volume_uuid: None,
        volume_path: None,
        volume_network: None,
        media_bsd_name: None,
        media_bsd_major: None,
        media_bsd_minor: None,
        media_bsd_unit: None,
        media_content: None,
        media_kind: None,
        media_leaf: None,
        media_name: None,
        media_path: None,
        media_removable: None,
        media_ejectable: None,
        media_writable: None,
        media_type: None,
        media_uuid: None,
        whole_disk_media_uuid: None,
        media_whole: None,
        media_encrypted: None,
        media_block_size_bytes: None,
        media_size_bytes: None,
        device_internal: None,
        device_model: None,
        device_path: None,
        device_protocol: None,
        device_vendor: None,
        reason: Some(reason.into()),
    }
}

fn unavailable_resource_values(
    status: NativeVolumeStatus,
    reason: impl Into<String>,
) -> NativeVolumeResourceValues {
    NativeVolumeResourceValues {
        status,
        is_automounted: None,
        is_browsable: None,
        is_ejectable: None,
        is_internal: None,
        is_local: None,
        is_read_only: None,
        is_reachable: None,
        is_removable: None,
        remount_url: None,
        supports_case_preserved_names: None,
        supports_case_sensitive_names: None,
        volume_uuid: None,
        reason: Some(reason.into()),
    }
}

fn unavailable_mount_table_entry(
    status: NativeVolumeStatus,
    reason: impl Into<String>,
) -> NativeVolumeMountTableEntry {
    NativeVolumeMountTableEntry {
        status,
        mount_point: None,
        mounted_from: None,
        filesystem_type: None,
        flags: None,
        is_read_only: None,
        is_local: None,
        reason: Some(reason.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_root_volume_description() {
        let description = copy_volume_description_for_path(Path::new("/"));

        assert_eq!(description.status, NativeVolumeStatus::Available);
        assert!(description.volume_path.is_some());
        assert!(
            description.volume_name.is_some()
                || description.volume_kind.is_some()
                || description.media_bsd_name.is_some()
        );
    }

    #[test]
    fn reports_missing_paths_without_diskarbitration_call() {
        let description = copy_volume_description_for_path(Path::new(
            "/tmp/gfm-native-volume-description-missing",
        ));

        assert_eq!(description.status, NativeVolumeStatus::Missing);
        assert!(description.reason.unwrap().contains("does not exist"));
    }

    #[test]
    fn resolves_root_volume_resource_values() {
        let values = copy_volume_resource_values(Path::new("/"));

        assert_eq!(values.status, NativeVolumeStatus::Available);
        assert!(values.is_local.is_some() || values.is_read_only.is_some());
        assert_eq!(values.is_reachable, Some(true));
        assert!(values.is_browsable.is_some() || values.volume_uuid.is_some());
    }

    #[test]
    fn resolves_root_mount_table_entry() {
        let entry = copy_volume_mount_table_entry(Path::new("/"));

        assert_eq!(entry.status, NativeVolumeStatus::Available);
        assert!(entry.mount_point.is_some());
        assert!(entry.filesystem_type.is_some());
        assert!(entry.flags.is_some());
    }

    #[test]
    fn resolves_current_mount_table_snapshot() {
        let table = copy_volume_mount_table();

        assert_eq!(table.status, NativeVolumeStatus::Available);
        assert!(table.entries.iter().any(|entry| {
            entry.mount_point.as_deref() == Some(Path::new("/"))
                && entry.filesystem_type.is_some()
                && entry.flags.is_some()
        }));
    }

    #[test]
    fn volume_event_stream_owns_diskarbitration_session_lifecycle() {
        let stream = NativeVolumeEventStream::start();

        assert!(stream.is_attached() || stream.try_recv().is_some());
        drop(stream);
    }

    #[test]
    fn maps_diskarbitration_dissenter_codes_to_typed_operation_status() {
        assert_eq!(
            native_operation_status_for_dissenter(DA_RETURN_BUSY),
            NativeVolumeOperationStatus::Busy
        );
        assert_eq!(
            native_operation_status_for_dissenter(DA_RETURN_BAD_ARGUMENT),
            NativeVolumeOperationStatus::BadArgument
        );
        assert_eq!(
            native_operation_status_for_dissenter(DA_RETURN_EXCLUSIVE_ACCESS),
            NativeVolumeOperationStatus::ExclusiveAccess
        );
        assert_eq!(
            native_operation_status_for_dissenter(DA_RETURN_NO_RESOURCES),
            NativeVolumeOperationStatus::NoResources
        );
        assert_eq!(
            native_operation_status_for_dissenter(DA_RETURN_NOT_FOUND),
            NativeVolumeOperationStatus::NotFound
        );
        assert_eq!(
            native_operation_status_for_dissenter(DA_RETURN_NOT_PERMITTED),
            NativeVolumeOperationStatus::NotPermitted
        );
        assert_eq!(
            native_operation_status_for_dissenter(DA_RETURN_NOT_PRIVILEGED),
            NativeVolumeOperationStatus::NotPrivileged
        );
        assert_eq!(
            native_operation_status_for_dissenter(DA_RETURN_NOT_MOUNTED),
            NativeVolumeOperationStatus::NotMounted
        );
        assert_eq!(
            native_operation_status_for_dissenter(DA_RETURN_NOT_READY),
            NativeVolumeOperationStatus::NotReady
        );
        assert_eq!(
            native_operation_status_for_dissenter(DA_RETURN_NOT_WRITABLE),
            NativeVolumeOperationStatus::NotWritable
        );
        assert_eq!(
            native_operation_status_for_dissenter(DA_RETURN_UNSUPPORTED),
            NativeVolumeOperationStatus::Unsupported
        );
        assert_eq!(
            native_operation_status_for_dissenter(0xF8DA0001),
            NativeVolumeOperationStatus::Failed
        );
    }

    #[test]
    fn missing_volume_operation_does_not_submit_to_diskarbitration() {
        let result = submit_volume_operation(
            Path::new("/tmp/gfm-native-volume-operation-missing"),
            NativeVolumeOperation::Eject,
        );

        assert_eq!(result.operation, NativeVolumeOperation::Eject);
        assert_eq!(result.status, NativeVolumeOperationStatus::Missing);
        assert_eq!(result.dissenter_status, None);
        assert!(result.reason.unwrap().contains("does not exist"));
    }

    #[test]
    fn invalid_bsd_mount_identity_does_not_submit_to_diskarbitration() {
        let result = submit_volume_mount_by_bsd_name("not/a/disk");

        assert_eq!(result.operation, NativeVolumeOperation::Mount);
        assert_eq!(result.status, NativeVolumeOperationStatus::Unsupported);
        assert_eq!(result.dissenter_status, None);
        assert_eq!(
            result.reason.as_deref(),
            Some("diskarbitration-mount-requires-bsd-name")
        );
    }

    #[test]
    fn malformed_bsd_mount_identity_does_not_submit_to_diskarbitration() {
        for name in ["notadisk", "disk", "diskXs1", "disk4s", "disk4s1/evil"] {
            let result = submit_volume_mount_by_bsd_name(name);

            assert_eq!(result.operation, NativeVolumeOperation::Mount);
            assert_eq!(result.status, NativeVolumeOperationStatus::Unsupported);
            assert_eq!(result.dissenter_status, None);
            assert_eq!(
                result.reason.as_deref(),
                Some("diskarbitration-mount-requires-bsd-name")
            );
        }
    }

    #[test]
    fn bsd_mount_identity_accepts_disk_and_slice_forms() {
        assert!(valid_bsd_disk_name("disk4"));
        assert!(valid_bsd_disk_name("disk4s1"));
        assert!(!valid_bsd_disk_name("notadisk"));
    }

    #[test]
    fn volume_operation_callback_grace_window_stays_interactive() {
        assert_eq!(
            volume_operation_submitted_reason(),
            "submitted-to-diskarbitration-timeout-500ms"
        );
    }
}
