use core_foundation::array::CFArray;
use core_foundation::base::{kCFAllocatorDefault, CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::date::CFDate;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation::url::CFURL;
use core_foundation_sys::array::CFArrayRef;
#[cfg(test)]
use core_foundation_sys::base::CFTypeID;
use core_foundation_sys::base::CFTypeRef;
use libc::c_void;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::ptr::NonNull;

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn MDItemsCreateWithURLs(allocator: CFTypeRef, urls: CFArrayRef) -> CFArrayRef;
    fn MDItemsCopyAttributes(items: CFArrayRef, names: CFArrayRef) -> CFArrayRef;
    #[cfg(test)]
    fn MDItemGetTypeID() -> CFTypeID;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeSpotlightStatus {
    Available,
    Missing,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSpotlightSnapshot {
    pub status: NativeSpotlightStatus,
    pub attributes: BTreeMap<String, Vec<String>>,
    pub reason: Option<String>,
}

pub fn read_spotlight_attributes(
    path: &Path,
    keys: &[&str],
) -> Result<NativeSpotlightSnapshot, String> {
    Ok(read_spotlight_attributes_batch(&[path], keys)?
        .into_iter()
        .next()
        .expect("single-path batch should always return one snapshot"))
}

pub fn read_spotlight_attributes_batch(
    paths: &[&Path],
    keys: &[&str],
) -> Result<Vec<NativeSpotlightSnapshot>, String> {
    let mut snapshots = vec![unavailable("pending native Spotlight batch result"); paths.len()];
    let mut valid_paths = Vec::new();
    let mut urls = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        match path_to_url(path) {
            Ok(PathUrl::Ready(url)) => {
                valid_paths.push(index);
                urls.push(url);
            }
            Ok(PathUrl::Missing(reason)) => snapshots[index] = missing(reason),
            Err(reason) => snapshots[index] = unavailable(reason),
        }
    }
    if valid_paths.is_empty() {
        return Ok(snapshots);
    }
    let item_array = create_items(&urls)?;
    let value_array = copy_attributes(&item_array, keys)?;
    let values = value_array.get_all_values();
    for (local_index, snapshot_index) in valid_paths.into_iter().enumerate() {
        snapshots[snapshot_index] = values
            .get(local_index)
            .and_then(|raw| cf_type_from_raw(*raw))
            .and_then(|value| values_by_key(&value, keys))
            .unwrap_or_else(|| unavailable("Metadata.framework did not return attributes"));
    }
    Ok(snapshots)
}

fn path_to_url(path: &Path) -> Result<PathUrl, String> {
    path.to_str()
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))?;
    match path.try_exists() {
        Ok(true) => {}
        Ok(false) => {
            return Ok(PathUrl::Missing(format!(
                "path does not exist: {}",
                path.display()
            )));
        }
        Err(error) => {
            return Err(format!(
                "path existence unavailable: {error}: {}",
                path.display()
            ));
        }
    }
    let metadata = fs::metadata(path)
        .map_err(|error| format!("path metadata unavailable: {error}: {}", path.display()))?;
    let url = CFURL::from_path(path, metadata.is_dir())
        .ok_or_else(|| format!("invalid path URL: {}", path.display()))?;
    Ok(PathUrl::Ready(url))
}

fn create_items(urls: &[CFURL]) -> Result<CFArray, String> {
    let urls = CFArray::from_CFTypes(urls).into_untyped();
    let raw = unsafe { MDItemsCreateWithURLs(kCFAllocatorDefault, urls.as_concrete_TypeRef()) };
    NonNull::new(raw as *mut c_void)
        .map(|raw| unsafe { CFArray::wrap_under_create_rule(raw.as_ptr() as CFArrayRef) })
        .ok_or_else(|| "Metadata.framework did not return a batched item array".to_string())
}

fn copy_attributes(item_array: &CFArray, keys: &[&str]) -> Result<CFArray, String> {
    let keys = keys
        .iter()
        .map(|key| CFString::new(key))
        .collect::<Vec<_>>();
    let keys = CFArray::from_CFTypes(&keys).into_untyped();
    let raw = unsafe {
        MDItemsCopyAttributes(item_array.as_concrete_TypeRef(), keys.as_concrete_TypeRef())
    };
    NonNull::new(raw as *mut c_void)
        .map(|raw| unsafe { CFArray::wrap_under_create_rule(raw.as_ptr() as CFArrayRef) })
        .ok_or_else(|| "Metadata.framework did not return batched attributes".to_string())
}

fn values_by_key(value: &CFType, keys: &[&str]) -> Option<NativeSpotlightSnapshot> {
    let values = value.downcast::<CFArray>()?;
    let attributes = values
        .get_all_values()
        .into_iter()
        .zip(keys.iter().copied())
        .filter_map(|(raw, key)| {
            cf_type_from_raw(raw).and_then(|value| {
                let values = collect_values(&value);
                (!values.is_empty()).then(|| (key.to_string(), values))
            })
        })
        .collect();
    Some(available(attributes))
}

fn available(attributes: BTreeMap<String, Vec<String>>) -> NativeSpotlightSnapshot {
    NativeSpotlightSnapshot {
        status: NativeSpotlightStatus::Available,
        attributes,
        reason: None,
    }
}

fn missing(reason: impl Into<String>) -> NativeSpotlightSnapshot {
    NativeSpotlightSnapshot {
        status: NativeSpotlightStatus::Missing,
        attributes: BTreeMap::new(),
        reason: Some(reason.into()),
    }
}

fn unavailable(reason: impl Into<String>) -> NativeSpotlightSnapshot {
    NativeSpotlightSnapshot {
        status: NativeSpotlightStatus::Unavailable,
        attributes: BTreeMap::new(),
        reason: Some(reason.into()),
    }
}

fn cf_type_from_raw(raw: *const c_void) -> Option<CFType> {
    NonNull::new(raw as *mut c_void)
        .map(|raw| unsafe { CFType::wrap_under_get_rule(raw.as_ptr() as CFTypeRef) })
}

fn collect_values(value: &CFType) -> Vec<String> {
    if let Some(string) = value.downcast::<CFString>() {
        return non_empty(string.to_string()).into_iter().collect();
    }
    if let Some(array) = value.downcast::<CFArray>() {
        return array
            .get_all_values()
            .into_iter()
            .filter_map(|raw| {
                NonNull::new(raw as *mut c_void)
                    .map(|raw| unsafe { CFType::wrap_under_get_rule(raw.as_ptr() as CFTypeRef) })
            })
            .flat_map(|nested| collect_values(&nested))
            .collect();
    }
    if let Some(date) = value.downcast::<CFDate>() {
        return vec![format!("{:.6}", date.abs_time())];
    }
    if let Some(number) = value.downcast::<CFNumber>() {
        return number_value(&number).into_iter().collect();
    }
    if let Some(boolean) = value.downcast::<CFBoolean>() {
        return vec![bool::from(boolean).to_string()];
    }
    Vec::new()
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn number_value(number: &CFNumber) -> Option<String> {
    number
        .to_i64()
        .map(|value| value.to_string())
        .or_else(|| number.to_f64().map(|value| value.to_string()))
}

enum PathUrl {
    Ready(CFURL),
    Missing(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_values_are_normalized() {
        let value = CFString::new("Report.md").into_CFType();

        assert_eq!(collect_values(&value), vec!["Report.md"]);
    }

    #[test]
    fn empty_strings_are_discarded() {
        let value = CFString::new("").into_CFType();

        assert!(collect_values(&value).is_empty());
    }

    #[test]
    fn arrays_are_flattened_to_strings() {
        let values = CFArray::from_CFTypes(&[CFString::new("Important"), CFString::new("Client")])
            .into_CFType();

        assert_eq!(collect_values(&values), vec!["Important", "Client"]);
    }

    #[test]
    fn dates_and_numbers_are_preserved() {
        let date = CFDate::new(42.25).into_CFType();
        let number = CFNumber::from(9_i64).into_CFType();

        assert_eq!(collect_values(&date), vec!["42.250000"]);
        assert_eq!(collect_values(&number), vec!["9"]);
    }

    #[test]
    fn mditem_type_is_available_on_macos() {
        assert_ne!(unsafe { MDItemGetTypeID() }, 0);
    }

    #[test]
    fn batch_reader_preserves_order_and_missing_entries() {
        let first = std::env::temp_dir().join(format!(
            "gfm-mac-sys-spotlight-missing-a-{}",
            std::process::id()
        ));
        let second = std::env::temp_dir().join(format!(
            "gfm-mac-sys-spotlight-missing-b-{}",
            std::process::id()
        ));

        let snapshots = read_spotlight_attributes_batch(
            &[first.as_path(), second.as_path()],
            &["kMDItemDisplayName", "kMDItemContentType"],
        )
        .unwrap();

        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].status, NativeSpotlightStatus::Missing);
        assert_eq!(snapshots[1].status, NativeSpotlightStatus::Missing);
        assert_ne!(snapshots[0].reason, snapshots[1].reason);
        assert!(snapshots[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("path does not exist"));
        assert!(snapshots[1]
            .reason
            .as_deref()
            .unwrap()
            .contains("path does not exist"));
    }

    #[test]
    fn path_probe_errors_are_unavailable_not_missing() {
        let path = std::env::temp_dir().join("s".repeat(300));

        let snapshot =
            read_spotlight_attributes(&path, &["kMDItemDisplayName", "kMDItemContentType"])
                .unwrap();

        assert_eq!(snapshot.status, NativeSpotlightStatus::Unavailable);
        assert!(snapshot.attributes.is_empty());
        assert!(snapshot
            .reason
            .as_deref()
            .unwrap()
            .contains("path existence unavailable"));
    }
}
