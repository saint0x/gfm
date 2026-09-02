use crate::url::{existing_path_url, NativePathUrl};
use core_foundation::base::{kCFAllocatorDefault, CFType, TCFType};
use core_foundation::boolean::{CFBoolean, CFBooleanRef};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation::url::CFURL;
use core_foundation_sys::array::CFArrayRef;
use core_foundation_sys::base::{CFAllocatorRef, CFGetTypeID, CFRelease, CFTypeRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValueIfPresent, CFDictionaryRef};
use core_foundation_sys::error::{CFErrorCopyDescription, CFErrorRef};
use core_foundation_sys::runloop::{
    kCFRunLoopDefaultMode, CFRunLoopGetCurrent, CFRunLoopRef, CFRunLoopRunInMode, CFRunLoopStop,
    CFRunLoopWakeUp,
};
use core_foundation_sys::string::CFStringRef;
use core_foundation_sys::url::CFURLRef;
use core_foundation_sys::uuid::{CFUUIDCreateString, CFUUIDRef};
use libc::{c_void, statfs, MNT_LOCAL, MNT_NOWAIT, MNT_RDONLY};
use objc::runtime::{Object, Sel, BOOL};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
const DA_RETURN_ERROR: u32 = 0xF8DA0001;
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
const MACH_ERROR_SYSTEM_SHIFT: u32 = 26;
const MACH_ERROR_SUBSYSTEM_SHIFT: u32 = 14;
const MACH_ERROR_SYSTEM_MASK: u32 = 0x3f;
const MACH_ERROR_SUBSYSTEM_MASK: u32 = 0xfff;
const MACH_ERROR_CODE_MASK: u32 = 0x3fff;
const MACH_ERROR_UNIX_SYSTEM: u32 = 0;
const MACH_ERROR_UNIX_SUBSYSTEM: u32 = 3;
const VOLUME_EVENT_STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const VOLUME_OPERATION_CALLBACK_TIMEOUT_SECONDS: f64 = 0.5;
const VOLUME_OPERATION_CALLBACK_TIMEOUT_MILLIS: u64 =
    (VOLUME_OPERATION_CALLBACK_TIMEOUT_SECONDS * 1000.0) as u64;
const VOLUME_OPERATION_CONTEXT_RETENTION: Duration = Duration::from_secs(60);
static NEXT_VOLUME_OPERATION_CONTEXT_ID: AtomicU64 = AtomicU64::new(1);
static VOLUME_OPERATION_CONTEXTS: OnceLock<Mutex<HashMap<u64, NativeVolumeOperationContext>>> =
    OnceLock::new();

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
    static NSURLVolumeIsEncryptedKey: CFStringRef;
    static NSURLVolumeIsInternalKey: CFStringRef;
    static NSURLVolumeIsLocalKey: CFStringRef;
    static NSURLVolumeIsReadOnlyKey: CFStringRef;
    static NSURLVolumeIsRemovableKey: CFStringRef;
    static NSURLVolumeIsRootFileSystemKey: CFStringRef;
    static NSURLVolumeURLForRemountingKey: CFStringRef;
    static NSURLVolumeUUIDStringKey: CFStringRef;
    static NSURLVolumeSupportsCasePreservedNamesKey: CFStringRef;
    static NSURLVolumeSupportsCaseSensitiveNamesKey: CFStringRef;
    static NSURLVolumeSupportsFileCloningKey: CFStringRef;
    static NSURLVolumeSupportsHardLinksKey: CFStringRef;
    static NSURLVolumeSupportsSparseFilesKey: CFStringRef;

    fn CFURLCopyResourcePropertyForKey(
        url: CFURLRef,
        key: CFStringRef,
        property_value_type_ref_ptr: *mut CFTypeRef,
        error: *mut CFErrorRef,
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
    pub is_encrypted: Option<bool>,
    pub is_internal: Option<bool>,
    pub is_local: Option<bool>,
    pub is_read_only: Option<bool>,
    pub is_reachable: Option<bool>,
    pub is_removable: Option<bool>,
    pub is_root_file_system: Option<bool>,
    pub remount_url: Option<String>,
    pub supports_case_preserved_names: Option<bool>,
    pub supports_case_sensitive_names: Option<bool>,
    pub supports_file_cloning: Option<bool>,
    pub supports_hard_links: Option<bool>,
    pub supports_sparse_files: Option<bool>,
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
    Error,
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
    Cancelled,
    Failed,
    Missing,
    Unavailable,
}

impl NativeVolumeOperationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Submitted => "submitted",
            Self::Error => "error",
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
            Self::Cancelled => "cancelled",
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
    run_loop: usize,
    sender: mpsc::Sender<NativeVolumeOperationResult>,
    created_at: Instant,
}

impl NativeVolumeEventStream {
    pub fn start() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let startup_event_tx = event_tx.clone();
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

        let (run_loop, thread) = match ready_rx.recv_timeout(VOLUME_EVENT_STARTUP_TIMEOUT) {
            Ok(run_loop) => (run_loop, Some(thread)),
            Err(RecvTimeoutError::Timeout) => {
                stop.store(true, Ordering::Release);
                let _ = startup_event_tx.send(NativeVolumeEvent {
                    kind: NativeVolumeEventKind::Unavailable,
                    description: unavailable("DiskArbitration event session startup timed out"),
                });
                (None, None)
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = startup_event_tx.send(NativeVolumeEvent {
                    kind: NativeVolumeEventKind::Unavailable,
                    description: unavailable("DiskArbitration event session exited before startup"),
                });
                (None, Some(thread))
            }
        };
        Self {
            receiver: event_rx,
            run_loop,
            stop,
            thread,
        }
    }

    pub fn detached() -> Self {
        let (_event_tx, event_rx) = mpsc::channel();
        Self {
            receiver: event_rx,
            run_loop: None,
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
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
    match path.try_exists() {
        Ok(true) => {}
        Ok(false) => return missing(format!("volume path does not exist: {}", path.display())),
        Err(err) => {
            return unavailable(format!(
                "volume path existence unavailable: {}: {err}",
                path.display()
            ));
        }
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

fn volume_operation_contexts() -> &'static Mutex<HashMap<u64, NativeVolumeOperationContext>> {
    VOLUME_OPERATION_CONTEXTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_volume_operation_context(
    operation: NativeVolumeOperation,
    run_loop: CFRunLoopRef,
    sender: mpsc::Sender<NativeVolumeOperationResult>,
) -> u64 {
    let now = Instant::now();
    cleanup_expired_volume_operation_contexts(now);
    let mut id = NEXT_VOLUME_OPERATION_CONTEXT_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        id = NEXT_VOLUME_OPERATION_CONTEXT_ID.fetch_add(1, Ordering::Relaxed);
    }
    let context = NativeVolumeOperationContext {
        operation,
        run_loop: run_loop as usize,
        sender,
        created_at: now,
    };
    volume_operation_contexts()
        .lock()
        .expect("volume operation context registry poisoned")
        .insert(id, context);
    id
}

fn take_volume_operation_context(id: u64) -> Option<NativeVolumeOperationContext> {
    volume_operation_contexts()
        .lock()
        .expect("volume operation context registry poisoned")
        .remove(&id)
}

#[cfg(test)]
fn volume_operation_context_is_pending(id: u64) -> bool {
    volume_operation_contexts()
        .lock()
        .expect("volume operation context registry poisoned")
        .contains_key(&id)
}

fn cleanup_expired_volume_operation_contexts(now: Instant) {
    volume_operation_contexts()
        .lock()
        .expect("volume operation context registry poisoned")
        .retain(|_, context| {
            now.checked_duration_since(context.created_at)
                .is_none_or(|age| age < VOLUME_OPERATION_CONTEXT_RETENTION)
        });
}

pub fn submit_volume_operation(
    path: &Path,
    operation: NativeVolumeOperation,
) -> NativeVolumeOperationResult {
    match path.try_exists() {
        Ok(true) => {}
        Ok(false) => {
            return NativeVolumeOperationResult {
                operation,
                status: NativeVolumeOperationStatus::Missing,
                dissenter_status: None,
                reason: Some(format!("volume path does not exist: {}", path.display())),
            };
        }
        Err(err) => {
            return NativeVolumeOperationResult {
                operation,
                status: NativeVolumeOperationStatus::Unavailable,
                dissenter_status: None,
                reason: Some(format!(
                    "volume path existence unavailable: {}: {err}",
                    path.display()
                )),
            };
        }
    }
    let Some((session, disk)) = create_disk_for_volume_path(path) else {
        return NativeVolumeOperationResult {
            operation,
            status: NativeVolumeOperationStatus::Unavailable,
            dissenter_status: None,
            reason: Some(format!(
                "DiskArbitration did not return a disk for {}",
                path.display()
            )),
        };
    };

    let run_loop = unsafe { CFRunLoopGetCurrent() };
    let (tx, rx) = mpsc::channel();
    let context_id = register_volume_operation_context(operation, run_loop, tx);
    let callback_context = context_id as usize as *mut c_void;
    unsafe {
        DASessionScheduleWithRunLoop(session, run_loop, kCFRunLoopDefaultMode);
    }
    match operation {
        NativeVolumeOperation::Eject => unsafe {
            DADiskEject(disk, 0, Some(volume_operation_callback), callback_context);
        },
        NativeVolumeOperation::Mount => unsafe {
            DADiskMount(
                disk,
                ptr::null(),
                0,
                Some(volume_operation_callback),
                callback_context,
            );
        },
        NativeVolumeOperation::Unmount => unsafe {
            DADiskUnmount(disk, 0, Some(volume_operation_callback), callback_context);
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
    let result = finish_volume_operation_context(rx, operation);

    unsafe {
        CFRelease(disk as CFTypeRef);
        CFRelease(session as CFTypeRef);
    }

    result
}

pub fn submit_volume_mount_by_bsd_name(bsd_name: &str) -> NativeVolumeOperationResult {
    let operation = NativeVolumeOperation::Mount;
    let bsd_name = bsd_name.trim();
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

    let bsd_name = match CString::new(bsd_name) {
        Ok(name) => name,
        Err(_) => {
            unsafe {
                CFRelease(session as CFTypeRef);
            }
            return NativeVolumeOperationResult {
                operation,
                status: NativeVolumeOperationStatus::Unsupported,
                dissenter_status: None,
                reason: Some("diskarbitration-mount-requires-bsd-name".to_string()),
            };
        }
    };
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
    let context_id = register_volume_operation_context(operation, run_loop, tx);
    let callback_context = context_id as usize as *mut c_void;
    unsafe {
        DASessionScheduleWithRunLoop(session, run_loop, kCFRunLoopDefaultMode);
        DADiskMount(
            disk,
            ptr::null(),
            0,
            Some(volume_operation_callback),
            callback_context,
        );
        CFRunLoopRunInMode(
            kCFRunLoopDefaultMode,
            VOLUME_OPERATION_CALLBACK_TIMEOUT_SECONDS,
            0,
        );
        DASessionUnscheduleFromRunLoop(session, run_loop, kCFRunLoopDefaultMode);
    }
    let result = finish_volume_operation_context(rx, operation);

    unsafe {
        CFRelease(disk as CFTypeRef);
        CFRelease(session as CFTypeRef);
    }

    result
}

fn valid_bsd_disk_name(name: &str) -> bool {
    let name = name.trim();
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

fn finish_volume_operation_context(
    rx: Receiver<NativeVolumeOperationResult>,
    operation: NativeVolumeOperation,
) -> NativeVolumeOperationResult {
    rx.try_recv()
        .unwrap_or_else(|_| NativeVolumeOperationResult {
            operation,
            status: NativeVolumeOperationStatus::Submitted,
            dissenter_status: None,
            reason: Some(volume_operation_submitted_reason()),
        })
}

unsafe extern "C" fn volume_operation_callback(
    _disk: DADiskRef,
    dissenter: DADissenterRef,
    context: *mut c_void,
) {
    if context.is_null() {
        return;
    }
    let context_id = context as usize as u64;
    let Some(context) = take_volume_operation_context(context_id) else {
        return;
    };
    let (status, dissenter_status, reason) = if dissenter.is_null() {
        (
            NativeVolumeOperationStatus::Succeeded,
            None,
            Some("diskarbitration-operation-succeeded".to_string()),
        )
    } else {
        let code = DADissenterGetStatus(dissenter) as u32;
        let native_status = dissenter_status_string(dissenter);
        let operation_status =
            native_operation_status_for_dissenter_with_status(code, native_status.as_deref());
        let reason = dissenter_reason_for_status(operation_status, code, native_status.as_deref());
        (operation_status, Some(code), Some(reason))
    };
    let _ = context.sender.send(NativeVolumeOperationResult {
        operation: context.operation,
        status,
        dissenter_status,
        reason,
    });
    CFRunLoopStop(context.run_loop as CFRunLoopRef);
    CFRunLoopWakeUp(context.run_loop as CFRunLoopRef);
}

#[cfg(test)]
fn native_operation_status_for_dissenter(code: u32) -> NativeVolumeOperationStatus {
    native_operation_status_for_dissenter_with_status(code, None)
}

fn native_operation_status_for_dissenter_with_status(
    code: u32,
    native_status: Option<&str>,
) -> NativeVolumeOperationStatus {
    if native_status.is_some_and(dissenter_status_mentions_cancellation) {
        return NativeVolumeOperationStatus::Cancelled;
    }
    match code {
        DA_RETURN_ERROR => NativeVolumeOperationStatus::Error,
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
        _ => native_operation_status_for_unix_dissenter(code)
            .unwrap_or(NativeVolumeOperationStatus::Failed),
    }
}

fn dissenter_status_mentions_cancellation(status: &str) -> bool {
    let status = status.to_ascii_lowercase();
    status.contains("cancelled") || status.contains("canceled")
}

fn native_operation_status_for_unix_dissenter(code: u32) -> Option<NativeVolumeOperationStatus> {
    let system = (code >> MACH_ERROR_SYSTEM_SHIFT) & MACH_ERROR_SYSTEM_MASK;
    let subsystem = (code >> MACH_ERROR_SUBSYSTEM_SHIFT) & MACH_ERROR_SUBSYSTEM_MASK;
    if system != MACH_ERROR_UNIX_SYSTEM || subsystem != MACH_ERROR_UNIX_SUBSYSTEM {
        return None;
    }

    let errno = code & MACH_ERROR_CODE_MASK;
    if errno == libc::EBUSY as u32 || errno == libc::EAGAIN as u32 {
        Some(NativeVolumeOperationStatus::Busy)
    } else if errno == libc::EPERM as u32 || errno == libc::EACCES as u32 {
        Some(NativeVolumeOperationStatus::NotPermitted)
    } else if errno == libc::EROFS as u32 {
        Some(NativeVolumeOperationStatus::NotWritable)
    } else if errno == libc::ENOENT as u32 || errno == libc::ENXIO as u32 {
        Some(NativeVolumeOperationStatus::NotFound)
    } else if errno == libc::EINVAL as u32 {
        Some(NativeVolumeOperationStatus::BadArgument)
    } else if errno == libc::ENOTSUP as u32 || errno == libc::EOPNOTSUPP as u32 {
        Some(NativeVolumeOperationStatus::Unsupported)
    } else if errno == libc::ENOMEM as u32 {
        Some(NativeVolumeOperationStatus::NoResources)
    } else if errno == libc::EINTR as u32 || errno == libc::ECANCELED as u32 {
        Some(NativeVolumeOperationStatus::Cancelled)
    } else {
        None
    }
}

fn volume_operation_submitted_reason() -> String {
    format!("submitted-to-diskarbitration-timeout-{VOLUME_OPERATION_CALLBACK_TIMEOUT_MILLIS}ms")
}

fn dissenter_status_string(dissenter: DADissenterRef) -> Option<String> {
    let status = unsafe { DADissenterGetStatusString(dissenter) };
    if status.is_null() {
        return None;
    }
    let status = unsafe { CFString::wrap_under_get_rule(status) }.to_string();
    (!status.is_empty()).then_some(status)
}

fn dissenter_reason_for_status(
    status: NativeVolumeOperationStatus,
    code: u32,
    native_status: Option<&str>,
) -> String {
    match native_status {
        Some(native_status) => format!(
            "diskarbitration-{}:0x{code:08x}:{native_status}",
            status.as_str()
        ),
        None => format!("diskarbitration-{}:0x{code:08x}", status.as_str()),
    }
}

pub fn copy_volume_resource_values(path: &Path) -> NativeVolumeResourceValues {
    let url = match existing_path_url(path, "volume path") {
        NativePathUrl::Ready(url) => url,
        NativePathUrl::Missing(reason) => {
            return unavailable_resource_values(NativeVolumeStatus::Missing, reason);
        }
        NativePathUrl::Unavailable(reason) | NativePathUrl::Invalid(reason) => {
            return unavailable_resource_values(NativeVolumeStatus::Unavailable, reason);
        }
    };
    let url = url.as_concrete_TypeRef();
    let mut errors = Vec::new();

    let is_automounted = copy_resource_bool(
        url,
        unsafe { NSURLVolumeIsAutomountedKey },
        "NSURLVolumeIsAutomountedKey",
        &mut errors,
    );
    let is_browsable = copy_resource_bool(
        url,
        unsafe { NSURLVolumeIsBrowsableKey },
        "NSURLVolumeIsBrowsableKey",
        &mut errors,
    );
    let is_ejectable = copy_resource_bool(
        url,
        unsafe { NSURLVolumeIsEjectableKey },
        "NSURLVolumeIsEjectableKey",
        &mut errors,
    );
    let is_encrypted = copy_resource_bool(
        url,
        unsafe { NSURLVolumeIsEncryptedKey },
        "NSURLVolumeIsEncryptedKey",
        &mut errors,
    );
    let is_internal = copy_resource_bool(
        url,
        unsafe { NSURLVolumeIsInternalKey },
        "NSURLVolumeIsInternalKey",
        &mut errors,
    );
    let is_local = copy_resource_bool(
        url,
        unsafe { NSURLVolumeIsLocalKey },
        "NSURLVolumeIsLocalKey",
        &mut errors,
    );
    let is_read_only = copy_resource_bool(
        url,
        unsafe { NSURLVolumeIsReadOnlyKey },
        "NSURLVolumeIsReadOnlyKey",
        &mut errors,
    );
    let is_reachable = check_resource_is_reachable(url);
    let is_removable = copy_resource_bool(
        url,
        unsafe { NSURLVolumeIsRemovableKey },
        "NSURLVolumeIsRemovableKey",
        &mut errors,
    );
    let is_root_file_system = copy_resource_bool(
        url,
        unsafe { NSURLVolumeIsRootFileSystemKey },
        "NSURLVolumeIsRootFileSystemKey",
        &mut errors,
    );
    let remount_url = copy_resource_url_string(
        url,
        unsafe { NSURLVolumeURLForRemountingKey },
        "NSURLVolumeURLForRemountingKey",
        &mut errors,
    );
    let supports_case_preserved_names = copy_resource_bool(
        url,
        unsafe { NSURLVolumeSupportsCasePreservedNamesKey },
        "NSURLVolumeSupportsCasePreservedNamesKey",
        &mut errors,
    );
    let supports_case_sensitive_names = copy_resource_bool(
        url,
        unsafe { NSURLVolumeSupportsCaseSensitiveNamesKey },
        "NSURLVolumeSupportsCaseSensitiveNamesKey",
        &mut errors,
    );
    let supports_file_cloning = copy_resource_bool(
        url,
        unsafe { NSURLVolumeSupportsFileCloningKey },
        "NSURLVolumeSupportsFileCloningKey",
        &mut errors,
    );
    let supports_hard_links = copy_resource_bool(
        url,
        unsafe { NSURLVolumeSupportsHardLinksKey },
        "NSURLVolumeSupportsHardLinksKey",
        &mut errors,
    );
    let supports_sparse_files = copy_resource_bool(
        url,
        unsafe { NSURLVolumeSupportsSparseFilesKey },
        "NSURLVolumeSupportsSparseFilesKey",
        &mut errors,
    );
    let volume_uuid = copy_resource_string(
        url,
        unsafe { NSURLVolumeUUIDStringKey },
        "NSURLVolumeUUIDStringKey",
        &mut errors,
    );
    let has_values = is_automounted.is_some()
        || is_browsable.is_some()
        || is_ejectable.is_some()
        || is_encrypted.is_some()
        || is_internal.is_some()
        || is_local.is_some()
        || is_read_only.is_some()
        || is_reachable.is_some()
        || is_removable.is_some()
        || is_root_file_system.is_some()
        || remount_url.is_some()
        || supports_case_preserved_names.is_some()
        || supports_case_sensitive_names.is_some()
        || supports_file_cloning.is_some()
        || supports_hard_links.is_some()
        || supports_sparse_files.is_some()
        || volume_uuid.is_some();
    let status = volume_resource_status_for_values(has_values, &errors);
    let reason = (status == NativeVolumeStatus::Unavailable)
        .then(|| unavailable_volume_resource_values_reason(path, &errors));
    NativeVolumeResourceValues {
        status,
        is_automounted,
        is_browsable,
        is_ejectable,
        is_encrypted,
        is_internal,
        is_local,
        is_read_only,
        is_reachable,
        is_removable,
        is_root_file_system,
        remount_url,
        supports_case_preserved_names,
        supports_case_sensitive_names,
        supports_file_cloning,
        supports_hard_links,
        supports_sparse_files,
        volume_uuid,
        reason,
    }
}

pub fn copy_volume_mount_table_entry(path: &Path) -> NativeVolumeMountTableEntry {
    match path.try_exists() {
        Ok(true) => {}
        Ok(false) => {
            return unavailable_mount_table_entry(
                NativeVolumeStatus::Missing,
                format!("volume path does not exist: {}", path.display()),
            );
        }
        Err(err) => {
            return unavailable_mount_table_entry(
                NativeVolumeStatus::Unavailable,
                format!(
                    "volume path existence unavailable: {}: {err}",
                    path.display()
                ),
            );
        }
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
    if !(path.try_exists().ok()?) {
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

fn copy_resource_bool(
    url: CFURLRef,
    key: CFStringRef,
    key_name: &'static str,
    errors: &mut Vec<String>,
) -> Option<bool> {
    let value = copy_resource_value(url, key, key_name, errors)?;
    if unsafe { CFGetTypeID(value.as_CFTypeRef()) } != unsafe { CFBooleanGetTypeID() } {
        return None;
    }
    let typed = unsafe { CFBoolean::wrap_under_get_rule(value.as_CFTypeRef() as CFBooleanRef) };
    Some(typed.into())
}

fn copy_resource_string(
    url: CFURLRef,
    key: CFStringRef,
    key_name: &'static str,
    errors: &mut Vec<String>,
) -> Option<String> {
    copy_resource_value(url, key, key_name, errors)?
        .downcast::<CFString>()
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
}

fn copy_resource_url_string(
    url: CFURLRef,
    key: CFStringRef,
    key_name: &'static str,
    errors: &mut Vec<String>,
) -> Option<String> {
    copy_resource_value(url, key, key_name, errors)?
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

fn copy_resource_value(
    url: CFURLRef,
    key: CFStringRef,
    key_name: &'static str,
    errors: &mut Vec<String>,
) -> Option<CFType> {
    let mut value: CFTypeRef = ptr::null();
    let mut error: CFErrorRef = ptr::null_mut();
    let copied = unsafe { CFURLCopyResourcePropertyForKey(url, key, &mut value, &mut error) };
    if copied == 0 || value.is_null() {
        if !error.is_null() {
            let description = unsafe { CFErrorCopyDescription(error) };
            let description = if description.is_null() {
                "resource value unavailable".to_string()
            } else {
                unsafe { CFString::wrap_under_create_rule(description) }.to_string()
            };
            unsafe {
                CFRelease(error as CFTypeRef);
            }
            errors.push(format!("{}={}", key_name, description));
        }
        None
    } else {
        Some(unsafe { CFType::wrap_under_create_rule(value) })
    }
}

fn unavailable_volume_resource_values_reason(path: &Path, errors: &[String]) -> String {
    let details = if errors.is_empty() {
        "no resource values returned".to_string()
    } else {
        errors.join("; ")
    };
    format!(
        "native volume URL resource values unavailable for {}: {}",
        path.display(),
        details
    )
}

fn volume_resource_status_for_values(has_values: bool, errors: &[String]) -> NativeVolumeStatus {
    if !has_values && !errors.is_empty() {
        NativeVolumeStatus::Unavailable
    } else {
        NativeVolumeStatus::Available
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
        is_encrypted: None,
        is_internal: None,
        is_local: None,
        is_read_only: None,
        is_reachable: None,
        is_removable: None,
        is_root_file_system: None,
        remount_url: None,
        supports_case_preserved_names: None,
        supports_case_sensitive_names: None,
        supports_file_cloning: None,
        supports_hard_links: None,
        supports_sparse_files: None,
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
    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

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

    #[cfg(unix)]
    #[test]
    fn volume_description_surfaces_path_probe_errors_as_unavailable() {
        let path = invalid_path("gfm-native-volume-description-invalid");

        let description = copy_volume_description_for_path(&path);

        assert_eq!(description.status, NativeVolumeStatus::Unavailable);
        assert!(description
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("path existence unavailable"));
    }

    #[test]
    fn resolves_root_volume_resource_values() {
        let values = copy_volume_resource_values(Path::new("/"));

        assert_eq!(values.status, NativeVolumeStatus::Available);
        assert!(values.is_local.is_some() || values.is_read_only.is_some());
        assert_eq!(values.is_reachable, Some(true));
        assert_eq!(values.is_root_file_system, Some(true));
        assert!(values.supports_file_cloning.is_some());
        assert!(values.supports_hard_links.is_some());
        assert!(values.supports_sparse_files.is_some());
        assert!(values.is_browsable.is_some() || values.volume_uuid.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn volume_resource_values_surface_path_probe_errors_as_unavailable() {
        let path = invalid_path("gfm-native-volume-resource-invalid");

        let values = copy_volume_resource_values(&path);

        assert_eq!(values.status, NativeVolumeStatus::Unavailable);
        assert!(values
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("path existence unavailable"));
    }

    #[test]
    fn volume_resource_status_reports_unavailable_only_when_all_values_fail() {
        let errors = vec!["NSURLVolumeIsLocalKey=Operation not permitted".to_string()];

        assert_eq!(
            volume_resource_status_for_values(false, &errors),
            NativeVolumeStatus::Unavailable
        );
        assert_eq!(
            volume_resource_status_for_values(true, &errors),
            NativeVolumeStatus::Available
        );
        assert_eq!(
            unavailable_volume_resource_values_reason(Path::new("/Volumes/Remote"), &errors),
            "native volume URL resource values unavailable for /Volumes/Remote: NSURLVolumeIsLocalKey=Operation not permitted"
        );
    }

    #[test]
    fn resolves_root_mount_table_entry() {
        let entry = copy_volume_mount_table_entry(Path::new("/"));

        assert_eq!(entry.status, NativeVolumeStatus::Available);
        assert!(entry.mount_point.is_some());
        assert!(entry.filesystem_type.is_some());
        assert!(entry.flags.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn mount_table_entry_surfaces_path_probe_errors_as_unavailable() {
        let path = invalid_path("gfm-native-volume-mount-table-invalid");

        let entry = copy_volume_mount_table_entry(&path);

        assert_eq!(entry.status, NativeVolumeStatus::Unavailable);
        assert!(entry
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("path existence unavailable"));
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
            native_operation_status_for_dissenter(DA_RETURN_ERROR),
            NativeVolumeOperationStatus::Error
        );
    }

    #[test]
    fn maps_bsd_encoded_dissenter_codes_to_typed_operation_status() {
        fn unix_dissenter_code(errno: i32) -> u32 {
            (MACH_ERROR_UNIX_SYSTEM << MACH_ERROR_SYSTEM_SHIFT)
                | (MACH_ERROR_UNIX_SUBSYSTEM << MACH_ERROR_SUBSYSTEM_SHIFT)
                | errno as u32
        }

        for errno in [libc::EBUSY, libc::EAGAIN] {
            assert_eq!(
                native_operation_status_for_dissenter(unix_dissenter_code(errno)),
                NativeVolumeOperationStatus::Busy
            );
        }

        for errno in [libc::EPERM, libc::EACCES] {
            assert_eq!(
                native_operation_status_for_dissenter(unix_dissenter_code(errno)),
                NativeVolumeOperationStatus::NotPermitted
            );
        }

        assert_eq!(
            native_operation_status_for_dissenter(unix_dissenter_code(libc::EROFS)),
            NativeVolumeOperationStatus::NotWritable
        );

        for errno in [libc::ENOENT, libc::ENXIO] {
            assert_eq!(
                native_operation_status_for_dissenter(unix_dissenter_code(errno)),
                NativeVolumeOperationStatus::NotFound
            );
        }

        assert_eq!(
            native_operation_status_for_dissenter(unix_dissenter_code(libc::EINVAL)),
            NativeVolumeOperationStatus::BadArgument
        );
        assert_eq!(
            native_operation_status_for_dissenter(unix_dissenter_code(libc::ENOTSUP)),
            NativeVolumeOperationStatus::Unsupported
        );
        assert_eq!(
            native_operation_status_for_dissenter(unix_dissenter_code(libc::EOPNOTSUPP)),
            NativeVolumeOperationStatus::Unsupported
        );
        assert_eq!(
            native_operation_status_for_dissenter(unix_dissenter_code(libc::ENOMEM)),
            NativeVolumeOperationStatus::NoResources
        );
        for errno in [libc::EINTR, libc::ECANCELED] {
            assert_eq!(
                native_operation_status_for_dissenter(unix_dissenter_code(errno)),
                NativeVolumeOperationStatus::Cancelled
            );
        }
    }

    #[test]
    fn dissenter_reason_includes_typed_operation_status() {
        assert_eq!(
            dissenter_reason_for_status(
                NativeVolumeOperationStatus::BadArgument,
                DA_RETURN_BAD_ARGUMENT,
                None
            ),
            "diskarbitration-bad-argument:0xf8da0003"
        );
        assert_eq!(
            dissenter_reason_for_status(
                NativeVolumeOperationStatus::Busy,
                DA_RETURN_BUSY,
                Some("Resource busy")
            ),
            "diskarbitration-busy:0xf8da0002:Resource busy"
        );
    }

    #[test]
    fn volume_operation_dissenter_text_preserves_user_cancellation() {
        for native_status in ["Operation canceled by user", "Operation cancelled by user"] {
            assert_eq!(
                native_operation_status_for_dissenter_with_status(
                    DA_RETURN_ERROR,
                    Some(native_status)
                ),
                NativeVolumeOperationStatus::Cancelled
            );
            assert_eq!(
                dissenter_reason_for_status(
                    NativeVolumeOperationStatus::Cancelled,
                    DA_RETURN_ERROR,
                    Some(native_status)
                ),
                format!("diskarbitration-cancelled:0xf8da0001:{native_status}")
            );
        }
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

    #[cfg(unix)]
    #[test]
    fn volume_operation_surfaces_path_probe_errors_as_unavailable() {
        let path = invalid_path("gfm-native-volume-operation-invalid");

        let result = submit_volume_operation(&path, NativeVolumeOperation::Eject);

        assert_eq!(result.operation, NativeVolumeOperation::Eject);
        assert_eq!(result.status, NativeVolumeOperationStatus::Unavailable);
        assert_eq!(result.dissenter_status, None);
        assert!(result
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("path existence unavailable"));
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
        for name in [
            "notadisk",
            "disk",
            "diskXs1",
            "disk4s",
            "disk4s1/evil",
            "disk4\0s1",
        ] {
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
        assert!(valid_bsd_disk_name(" disk4 "));
        assert!(valid_bsd_disk_name("\tdisk4s1\n"));
        assert!(!valid_bsd_disk_name("notadisk"));
    }

    #[test]
    fn volume_operation_callback_grace_window_stays_interactive() {
        assert_eq!(
            volume_operation_submitted_reason(),
            "submitted-to-diskarbitration-timeout-500ms"
        );
    }

    #[test]
    fn volume_operation_timeout_keeps_callback_context_pending_as_submitted() {
        let (tx, rx) = mpsc::channel();
        let context_id = register_volume_operation_context(
            NativeVolumeOperation::Eject,
            unsafe { CFRunLoopGetCurrent() },
            tx,
        );

        let result = finish_volume_operation_context(rx, NativeVolumeOperation::Eject);

        assert_eq!(result.operation, NativeVolumeOperation::Eject);
        assert_eq!(result.status, NativeVolumeOperationStatus::Submitted);
        assert_eq!(result.reason, Some(volume_operation_submitted_reason()));
        assert!(volume_operation_context_is_pending(context_id));
        assert!(take_volume_operation_context(context_id).is_some());
    }

    #[test]
    fn volume_operation_callback_result_wins_and_consumes_callback_context() {
        let (tx, rx) = mpsc::channel();
        let context_id = register_volume_operation_context(
            NativeVolumeOperation::Unmount,
            unsafe { CFRunLoopGetCurrent() },
            tx,
        );
        let callback_context = context_id as usize as *mut c_void;
        assert!(volume_operation_context_is_pending(context_id));

        unsafe {
            volume_operation_callback(ptr::null(), ptr::null(), callback_context);
        }

        assert!(!volume_operation_context_is_pending(context_id));
        let result = finish_volume_operation_context(rx, NativeVolumeOperation::Unmount);

        assert_eq!(result.operation, NativeVolumeOperation::Unmount);
        assert_eq!(result.status, NativeVolumeOperationStatus::Succeeded);
        assert_eq!(
            result.reason.as_deref(),
            Some("diskarbitration-operation-succeeded")
        );
    }

    #[test]
    fn volume_operation_context_cleanup_drops_abandoned_pending_context() {
        let (tx, _rx) = mpsc::channel();
        let now = Instant::now();
        let context_id = register_volume_operation_context(
            NativeVolumeOperation::Mount,
            unsafe { CFRunLoopGetCurrent() },
            tx,
        );
        {
            let mut contexts = volume_operation_contexts()
                .lock()
                .expect("volume operation context registry poisoned");
            contexts
                .get_mut(&context_id)
                .expect("registered context")
                .created_at = now - VOLUME_OPERATION_CONTEXT_RETENTION - Duration::from_secs(1);
        }

        cleanup_expired_volume_operation_contexts(now);

        assert!(!volume_operation_context_is_pending(context_id));
        unsafe {
            volume_operation_callback(ptr::null(), ptr::null(), context_id as usize as *mut c_void);
        }
    }

    #[cfg(unix)]
    fn invalid_path(name: &str) -> PathBuf {
        PathBuf::from(OsString::from_vec(
            format!("/tmp/{name}\0path").into_bytes(),
        ))
    }
}
