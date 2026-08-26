use gfm_types::{GfmError, Result};
use std::fs::{self, File};
use std::io;
use std::path::Path;

pub(crate) fn preserve_metadata(from: &Path, to: &Path, metadata: &fs::Metadata) -> Result<()> {
    preserve_ownership(to, metadata)?;
    preserve_permissions(to, metadata)?;
    preserve_times(to, metadata)?;
    preserve_xattrs(from, to)?;
    preserve_acls(from, to)?;
    preserve_file_flags(to, metadata)
}

#[cfg(unix)]
fn preserve_ownership(to: &Path, metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    match rustix::fs::chown(
        to,
        Some(rustix::fs::Uid::from_raw(metadata.uid())),
        Some(rustix::fs::Gid::from_raw(metadata.gid())),
    ) {
        Ok(()) => Ok(()),
        Err(err) => {
            let err = io::Error::from(err);
            if ownership_preservation_unsupported(&err) {
                Ok(())
            } else {
                Err(GfmError::io(to, err))
            }
        }
    }
}

#[cfg(unix)]
fn ownership_preservation_unsupported(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::EPERM) | Some(libc::EACCES) | Some(libc::ENOTSUP)
    ) || matches!(
        err.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported
    )
}

#[cfg(not(unix))]
fn preserve_ownership(_to: &Path, _metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}

fn preserve_permissions(to: &Path, metadata: &fs::Metadata) -> Result<()> {
    fs::set_permissions(to, metadata.permissions()).map_err(|err| GfmError::io(to, err))
}

fn preserve_times(to: &Path, metadata: &fs::Metadata) -> Result<()> {
    let atime = filetime::FileTime::from_last_access_time(metadata);
    let mtime = filetime::FileTime::from_last_modification_time(metadata);
    filetime::set_file_times(to, atime, mtime).map_err(|err| GfmError::io(to, err))?;
    preserve_creation_time(to, metadata)
}

pub(crate) fn preserve_symlink_times(to: &Path, metadata: &fs::Metadata) -> Result<()> {
    let atime = filetime::FileTime::from_last_access_time(metadata);
    let mtime = filetime::FileTime::from_last_modification_time(metadata);
    match filetime::set_symlink_file_times(to, atime, mtime) {
        Ok(()) => Ok(()),
        Err(err) if time_preservation_unsupported(&err) => Ok(()),
        Err(err) => Err(GfmError::io(to, err)),
    }
}

#[cfg(target_vendor = "apple")]
fn preserve_creation_time(to: &Path, metadata: &fs::Metadata) -> Result<()> {
    use std::os::darwin::fs::FileTimesExt;

    let created = match metadata.created() {
        Ok(created) => created,
        Err(err) if time_preservation_unsupported(&err) => return Ok(()),
        Err(err) => return Err(GfmError::io(to, err)),
    };
    let file = File::open(to).map_err(|err| GfmError::io(to, err))?;
    let times = fs::FileTimes::new().set_created(created);
    match file.set_times(times) {
        Ok(()) => Ok(()),
        Err(err) if time_preservation_unsupported(&err) => Ok(()),
        Err(err) => Err(GfmError::io(to, err)),
    }
}

#[cfg(not(target_vendor = "apple"))]
fn preserve_creation_time(_to: &Path, _metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}

pub(crate) fn time_preservation_unsupported(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::ENOTSUP) | Some(libc::EPERM) | Some(libc::EACCES)
    ) || matches!(
        err.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
    )
}

pub(crate) fn preserve_xattrs(from: &Path, to: &Path) -> Result<()> {
    let names = match xattr::list(from) {
        Ok(names) => names,
        Err(err) if xattr_copy_unsupported(&err) => return Ok(()),
        Err(err) => return Err(GfmError::io(from, err)),
    };
    for name in names {
        let value = match xattr::get(from, &name) {
            Ok(Some(value)) => value,
            Ok(None) => continue,
            Err(err) if xattr_copy_unsupported(&err) => continue,
            Err(err) => return Err(GfmError::io(from, err)),
        };
        match xattr::set(to, &name, &value) {
            Ok(()) => {}
            Err(err) if xattr_copy_unsupported(&err) => {}
            Err(err) => return Err(GfmError::io(to, err)),
        }
    }
    Ok(())
}

pub(crate) fn xattr_copy_unsupported(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::ENOTSUP)
            | Some(libc::ENODATA)
            | Some(libc::ENOATTR)
            | Some(libc::EPERM)
            | Some(libc::EACCES)
    ) || matches!(
        err.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
    )
}

#[cfg(target_os = "macos")]
fn preserve_acls(from: &Path, to: &Path) -> Result<()> {
    let entries = match exacl::getfacl(from, None::<exacl::AclOption>) {
        Ok(entries) => entries,
        Err(err) if acl_copy_unsupported(&err) => return Ok(()),
        Err(err) => return Err(GfmError::io(from, err)),
    };
    if entries.is_empty() {
        return Ok(());
    }
    let paths = [to];
    match exacl::setfacl(&paths, &entries, None::<exacl::AclOption>) {
        Ok(()) => Ok(()),
        Err(err) if acl_copy_unsupported(&err) => Ok(()),
        Err(err) => Err(GfmError::io(to, err)),
    }
}

#[cfg(not(target_os = "macos"))]
fn preserve_acls(_from: &Path, _to: &Path) -> Result<()> {
    Ok(())
}

pub(crate) fn acl_copy_unsupported(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::ENOTSUP)
            | Some(libc::ENOSYS)
            | Some(libc::ENOENT)
            | Some(libc::EPERM)
            | Some(libc::EACCES)
    ) || matches!(
        err.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
    )
}

#[cfg(target_vendor = "apple")]
fn preserve_file_flags(to: &Path, metadata: &fs::Metadata) -> Result<()> {
    use nix::sys::stat::FileFlag;
    use std::os::darwin::fs::MetadataExt;

    let flags = metadata.st_flags();
    if flags == 0 || metadata.file_type().is_symlink() {
        return Ok(());
    }
    match nix::unistd::chflags(to, FileFlag::from_bits_retain(flags)) {
        Ok(()) => Ok(()),
        Err(err) => {
            let err = io::Error::from_raw_os_error(err as i32);
            if file_flag_preservation_unsupported(&err) {
                Ok(())
            } else {
                Err(GfmError::io(to, err))
            }
        }
    }
}

#[cfg(target_vendor = "apple")]
pub(crate) fn file_flag_preservation_unsupported(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::ENOTSUP) | Some(libc::EPERM) | Some(libc::EACCES) | Some(libc::EROFS)
    ) || matches!(
        err.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
    )
}

#[cfg(not(target_vendor = "apple"))]
fn preserve_file_flags(_to: &Path, _metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}
