use core_foundation::array::CFArray;
use core_foundation::base::{kCFAllocatorDefault, CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::date::CFDate;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::CFURL;
#[cfg(test)]
use core_foundation_sys::base::CFTypeID;
use core_foundation_sys::base::CFTypeRef;
use libc::c_void;
use std::collections::BTreeMap;
use std::path::Path;
use std::ptr::NonNull;

type MDItemRef = *const c_void;

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn MDItemCreate(allocator: CFTypeRef, path: CFStringRef) -> MDItemRef;
    fn MDItemCopyAttribute(item: MDItemRef, name: CFStringRef) -> CFTypeRef;
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
    let path_string = path
        .to_str()
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))?;
    if !path.exists() {
        return Ok(NativeSpotlightSnapshot {
            status: NativeSpotlightStatus::Missing,
            attributes: BTreeMap::new(),
            reason: Some(format!("path does not exist: {}", path.display())),
        });
    }
    let item = create_item(path, path_string)?;
    let attributes = keys
        .iter()
        .filter_map(|key| {
            copy_attribute(&item, key).and_then(|value| {
                let values = collect_values(&value);
                (!values.is_empty()).then(|| ((*key).to_string(), values))
            })
        })
        .collect();
    Ok(NativeSpotlightSnapshot {
        status: NativeSpotlightStatus::Available,
        attributes,
        reason: None,
    })
}

fn create_item(path: &Path, path_string: &str) -> Result<MDItem, String> {
    let cf_path = CFString::new(path_string);
    let raw = unsafe { MDItemCreate(kCFAllocatorDefault, cf_path.as_concrete_TypeRef()) };
    if let Some(raw) = NonNull::new(raw as *mut c_void) {
        return Ok(MDItem(raw.as_ptr()));
    }
    let url = CFURL::from_path(path, path.is_dir())
        .ok_or_else(|| format!("invalid path URL: {}", path.display()))?;
    let fallback = CFString::new(&url.get_string().to_string());
    let raw = unsafe { MDItemCreate(kCFAllocatorDefault, fallback.as_concrete_TypeRef()) };
    NonNull::new(raw as *mut c_void)
        .map(|raw| MDItem(raw.as_ptr()))
        .ok_or_else(|| {
            format!(
                "Metadata.framework did not return an item for {}",
                path.display()
            )
        })
}

fn copy_attribute(item: &MDItem, key: &str) -> Option<CFType> {
    let key = CFString::new(key);
    let raw = unsafe { MDItemCopyAttribute(item.0, key.as_concrete_TypeRef()) };
    NonNull::new(raw as *mut c_void)
        .map(|raw| unsafe { CFType::wrap_under_create_rule(raw.as_ptr() as CFTypeRef) })
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

struct MDItem(MDItemRef);

impl Drop for MDItem {
    fn drop(&mut self) {
        unsafe {
            core_foundation::base::CFRelease(self.0 as CFTypeRef);
        }
    }
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
}
