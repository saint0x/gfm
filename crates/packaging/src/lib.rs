use gfm_types::{GfmError, Result};
use plist::{Dictionary, Value};
use std::fs;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

mod artifact;
mod notarize;
mod path;
mod policy;
mod toolchain;

pub use artifact::{
    validate_release_artifact, GatekeeperAssessment, ReleaseArtifactReport, ReleaseArtifactSpec,
    SignatureStatus,
};
pub use notarize::{
    notarize_app_bundle, NotarizationCredentials, NotarizationSpec, NotarizationStatus,
    NotarizationTicket,
};
pub use policy::{
    CrashReportMode, CrashReportPolicy, DiagnosticMode, DiagnosticsPolicy, ReleaseChannel,
    ReleasePolicy, RollbackPolicy, UpdateDecision, UpdateMode, UpdatePolicy,
};
pub use toolchain::{
    require_codesign_toolchain, require_release_xcode_toolchain, AppleToolchainReport,
    AppleToolchainUtility,
};

const DEFAULT_MINIMUM_SYSTEM_VERSION: &str = "14.0";
const MAX_INFO_PLIST_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigningIdentity {
    AdHoc,
    DeveloperId(String),
    Unsigned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppBundleSpec {
    pub product_name: String,
    pub bundle_identifier: String,
    pub version: String,
    pub build: String,
    pub minimum_system_version: String,
    pub executable: PathBuf,
    pub icon: PathBuf,
    pub output_dir: PathBuf,
    pub signing_identity: SigningIdentity,
}

impl AppBundleSpec {
    pub fn new(
        executable: impl Into<PathBuf>,
        icon: impl Into<PathBuf>,
        output_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            product_name: "GFM".to_string(),
            bundle_identifier: "com.saint0x.gfm".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            build: env!("CARGO_PKG_VERSION").to_string(),
            minimum_system_version: DEFAULT_MINIMUM_SYSTEM_VERSION.to_string(),
            executable: executable.into(),
            icon: icon.into(),
            output_dir: output_dir.into(),
            signing_identity: SigningIdentity::AdHoc,
        }
    }

    pub fn app_path(&self) -> PathBuf {
        self.output_dir.join(format!("{}.app", self.product_name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppBundle {
    pub app_path: PathBuf,
    pub executable_path: PathBuf,
    pub info_plist_path: PathBuf,
    pub entitlements_path: PathBuf,
    pub icon_path: PathBuf,
    pub signed: bool,
}

pub fn build_app_bundle(spec: &AppBundleSpec) -> Result<AppBundle> {
    validate_spec(spec)?;
    let app_path = spec.app_path();
    let contents = app_path.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    recreate_dir(&app_path)?;
    create_dir(&macos)?;
    create_dir(&resources)?;

    let executable_path = macos.join("gfm");
    copy_file(&spec.executable, &executable_path)?;
    preserve_executable_mode(&spec.executable, &executable_path)?;

    let info_plist_path = contents.join("Info.plist");
    write_file(
        &info_plist_path,
        info_plist(
            &spec.product_name,
            &spec.bundle_identifier,
            &spec.version,
            &spec.build,
            &spec.minimum_system_version,
        ),
    )?;

    let entitlements_path = spec.output_dir.join("GFM.entitlements");
    write_file(&entitlements_path, entitlements_plist())?;

    let icon_path = resources.join("GFM.icns");
    copy_file(&spec.icon, &icon_path)?;

    let mut bundle = AppBundle {
        app_path,
        executable_path,
        info_plist_path,
        entitlements_path,
        icon_path,
        signed: false,
    };
    if spec.signing_identity != SigningIdentity::Unsigned {
        sign_app_bundle(spec, &bundle)?;
        bundle.signed = true;
    }
    validate_app_bundle(&bundle)?;
    Ok(bundle)
}

pub fn validate_app_bundle(bundle: &AppBundle) -> Result<()> {
    ensure_dir(&bundle.app_path.join("Contents"))?;
    ensure_dir(&bundle.app_path.join("Contents/MacOS"))?;
    ensure_dir(&bundle.app_path.join("Contents/Resources"))?;
    ensure_file(&bundle.executable_path)?;
    ensure_file(&bundle.info_plist_path)?;
    ensure_file(&bundle.entitlements_path)?;
    ensure_file(&bundle.icon_path)?;
    let plist = read_info_plist(&bundle.info_plist_path)?;
    for required in [
        "CFBundleExecutable",
        "CFBundleIdentifier",
        "CFBundleIconFile",
        "LSMinimumSystemVersion",
    ] {
        require_info_plist_string(&plist, &bundle.info_plist_path, required)?;
    }
    require_info_plist_key(&plist, &bundle.info_plist_path, "CFBundleDocumentTypes")?;
    if !info_plist_has_item_content_types(&plist) {
        return Err(missing_info_plist_required(
            &bundle.info_plist_path,
            "LSItemContentTypes",
        ));
    }
    for required in ["public.folder", "public.item"] {
        if !info_plist_contains_document_type(&plist, required) {
            return Err(missing_info_plist_required(
                &bundle.info_plist_path,
                required,
            ));
        }
    }
    if bundle.signed {
        verify_codesign(&bundle.app_path)?;
    }
    Ok(())
}

pub fn register_launch_services(app_path: impl AsRef<Path>) -> Result<()> {
    let app_path = app_path.as_ref();
    ensure_dir(app_path)?;
    let status = Command::new(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
    )
    .arg("-f")
    .arg(app_path)
    .status()
    .map_err(|err| GfmError::io(app_path, err))?;
    if status.success() {
        Ok(())
    } else {
        Err(GfmError::Format(format!(
            "Launch Services registration failed for {} with status {status}",
            app_path.display()
        )))
    }
}

fn validate_spec(spec: &AppBundleSpec) -> Result<()> {
    ensure_file(&spec.executable)?;
    ensure_file(&spec.icon)?;
    if spec.product_name.trim().is_empty() {
        return Err(GfmError::Format("product name cannot be empty".to_string()));
    }
    if spec.bundle_identifier.trim().is_empty() || !spec.bundle_identifier.contains('.') {
        return Err(GfmError::Format(
            "bundle identifier must be a reverse-DNS identifier".to_string(),
        ));
    }
    if spec.version.trim().is_empty() || spec.build.trim().is_empty() {
        return Err(GfmError::Format(
            "bundle version and build cannot be empty".to_string(),
        ));
    }
    Ok(())
}

fn sign_app_bundle(spec: &AppBundleSpec, bundle: &AppBundle) -> Result<()> {
    let identity = match &spec.signing_identity {
        SigningIdentity::AdHoc => "-",
        SigningIdentity::DeveloperId(identity) => identity.as_str(),
        SigningIdentity::Unsigned => return Ok(()),
    };
    let status = Command::new("codesign")
        .arg("--force")
        .arg("--sign")
        .arg(identity)
        .arg("--entitlements")
        .arg(&bundle.entitlements_path)
        .arg("--options")
        .arg("runtime")
        .arg(&bundle.app_path)
        .status()
        .map_err(|err| GfmError::io(&bundle.app_path, err))?;
    if status.success() {
        Ok(())
    } else {
        Err(GfmError::Format(format!(
            "codesign failed for {} with status {status}",
            bundle.app_path.display()
        )))
    }
}

fn verify_codesign(app_path: &Path) -> Result<()> {
    let status = Command::new("codesign")
        .arg("--verify")
        .arg("--strict")
        .arg(app_path)
        .status()
        .map_err(|err| GfmError::io(app_path, err))?;
    if status.success() {
        Ok(())
    } else {
        Err(GfmError::Format(format!(
            "codesign verification failed for {} with status {status}",
            app_path.display()
        )))
    }
}

fn info_plist(
    product_name: &str,
    bundle_identifier: &str,
    version: &str,
    build: &str,
    minimum_system_version: &str,
) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>{product_name}</string>
  <key>CFBundleExecutable</key>
  <string>gfm</string>
  <key>CFBundleIconFile</key>
  <string>GFM</string>
  <key>CFBundleIdentifier</key>
  <string>{bundle_identifier}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>{product_name}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>{version}</string>
  <key>CFBundleVersion</key>
  <string>{build}</string>
  <key>LSMinimumSystemVersion</key>
  <string>{minimum_system_version}</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.productivity</string>
  <key>NSSupportsAutomaticGraphicsSwitching</key>
  <true/>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key>
      <string>Folders</string>
      <key>CFBundleTypeRole</key>
      <string>Viewer</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.folder</string>
      </array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key>
      <string>Files</string>
      <key>CFBundleTypeRole</key>
      <string>Viewer</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.item</string>
        <string>public.data</string>
      </array>
    </dict>
  </array>
</dict>
</plist>
"#
    )
}

fn entitlements_plist() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.files.user-selected.read-write</key>
  <true/>
  <key>com.apple.security.files.bookmarks.app-scope</key>
  <true/>
  <key>com.apple.security.files.bookmarks.document-scope</key>
  <true/>
</dict>
</plist>
"#
}

fn recreate_dir(path: &Path) -> Result<()> {
    path::recreate_dir(path, "bundle")
}

fn create_dir(path: &Path) -> Result<()> {
    path::create_dir(path)
}

fn ensure_dir(path: &Path) -> Result<()> {
    path::ensure_dir(path, "bundle")
}

fn ensure_file(path: &Path) -> Result<()> {
    path::ensure_file(path, "bundle")
}

fn copy_file(from: &Path, to: &Path) -> Result<()> {
    fs::copy(from, to).map_err(|err| GfmError::io(to, err))?;
    Ok(())
}

fn write_file(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path, contents).map_err(|err| GfmError::io(path, err))
}

#[cfg(unix)]
fn preserve_executable_mode(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(from)
        .map_err(|err| GfmError::io(from, err))?
        .permissions()
        .mode();
    let mut permissions = fs::metadata(to)
        .map_err(|err| GfmError::io(to, err))?
        .permissions();
    permissions.set_mode(mode | 0o755);
    fs::set_permissions(to, permissions).map_err(|err| GfmError::io(to, err))
}

#[cfg(not(unix))]
fn preserve_executable_mode(_from: &Path, _to: &Path) -> Result<()> {
    Ok(())
}

pub(crate) fn read_info_plist(path: &Path) -> Result<Dictionary> {
    let len = fs::metadata(path)
        .map_err(|err| GfmError::io(path, err))?
        .len();
    if len > MAX_INFO_PLIST_BYTES {
        return Err(GfmError::Format(format!(
            "{} Info.plist exceeds bounded validation budget of {MAX_INFO_PLIST_BYTES} bytes",
            path.display()
        )));
    }
    let file = File::open(path).map_err(|err| GfmError::io(path, err))?;
    let reader = BufReader::new(file).take(MAX_INFO_PLIST_BYTES);
    let value = Value::from_reader(reader).map_err(|err| {
        GfmError::Format(format!(
            "{} could not parse Info.plist: {err}",
            path.display()
        ))
    })?;
    match value {
        Value::Dictionary(dictionary) => Ok(dictionary),
        _ => Err(GfmError::Format(format!(
            "{} Info.plist root is not a dictionary",
            path.display()
        ))),
    }
}

pub(crate) fn plist_string_value(plist: &Dictionary, key: &str) -> Result<String> {
    plist
        .get(key)
        .and_then(Value::as_string)
        .map(ToOwned::to_owned)
        .ok_or_else(|| GfmError::Format(format!("Info.plist missing string `{key}`")))
}

pub(crate) fn info_plist_contains_document_type(plist: &Dictionary, uti: &str) -> bool {
    plist
        .get("CFBundleDocumentTypes")
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry
                    .as_dictionary()
                    .and_then(|dictionary| dictionary.get("LSItemContentTypes"))
                    .and_then(Value::as_array)
                    .is_some_and(|types| types.iter().any(|value| value.as_string() == Some(uti)))
            })
        })
}

fn info_plist_has_item_content_types(plist: &Dictionary) -> bool {
    plist
        .get("CFBundleDocumentTypes")
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry
                    .as_dictionary()
                    .and_then(|dictionary| dictionary.get("LSItemContentTypes"))
                    .and_then(Value::as_array)
                    .is_some()
            })
        })
}

fn require_info_plist_string(plist: &Dictionary, path: &Path, key: &str) -> Result<()> {
    plist_string_value(plist, key)
        .map(|_| ())
        .map_err(|_| missing_info_plist_required(path, key))
}

fn require_info_plist_key(plist: &Dictionary, path: &Path, key: &str) -> Result<()> {
    if plist.contains_key(key) {
        Ok(())
    } else {
        Err(missing_info_plist_required(path, key))
    }
}

fn missing_info_plist_required(path: &Path, required: &str) -> GfmError {
    GfmError::Format(format!(
        "{} missing required Info.plist key or value `{required}`",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn builds_unsigned_finder_compatible_app_bundle_layout() {
        let root = temp_root("unsigned");
        let executable = std::env::current_exe().expect("current test executable");
        let icon = root.join("GFM.icns");
        fs::create_dir_all(&root).expect("create temp root");
        fs::write(&icon, b"icns-test").expect("write icon");

        let mut spec = AppBundleSpec::new(executable, &icon, root.join("dist"));
        spec.signing_identity = SigningIdentity::Unsigned;
        let bundle = build_app_bundle(&spec).expect("build app bundle");

        assert_eq!(bundle.app_path, root.join("dist/GFM.app"));
        assert!(bundle.executable_path.is_file());
        assert!(bundle.icon_path.is_file());
        assert!(bundle.entitlements_path.is_file());
        let info = fs::read_to_string(bundle.info_plist_path).expect("read Info.plist");
        assert!(info.contains("<string>com.saint0x.gfm</string>"));
        assert!(info.contains("<string>APPL</string>"));
        assert!(info.contains("<string>public.folder</string>"));
        assert!(info.contains("<string>public.item</string>"));
    }

    #[test]
    fn rejects_invalid_bundle_identifier() {
        let root = temp_root("invalid-id");
        fs::create_dir_all(&root).expect("create temp root");
        let icon = root.join("GFM.icns");
        fs::write(&icon, b"icns-test").expect("write icon");
        let executable = std::env::current_exe().expect("current test executable");
        let mut spec = AppBundleSpec::new(executable, icon, root.join("dist"));
        spec.bundle_identifier = "gfm".to_string();

        let err = build_app_bundle(&spec).expect_err("invalid bundle id fails");
        assert!(err.to_string().contains("reverse-DNS"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn builds_ad_hoc_signed_app_bundle() {
        let root = temp_root("signed");
        let executable = std::env::current_exe().expect("current test executable");
        let icon = root.join("GFM.icns");
        fs::create_dir_all(&root).expect("create temp root");
        fs::write(&icon, b"icns-test").expect("write icon");

        let spec = AppBundleSpec::new(executable, &icon, root.join("dist"));
        let bundle = build_app_bundle(&spec).expect("build signed app bundle");

        assert!(bundle.signed);
        validate_app_bundle(&bundle).expect("signed bundle validates");
    }

    #[test]
    fn validates_required_bundle_artifacts() {
        let root = temp_root("validate");
        let executable = std::env::current_exe().expect("current test executable");
        let icon = root.join("GFM.icns");
        fs::create_dir_all(&root).expect("create temp root");
        fs::write(&icon, b"icns-test").expect("write icon");
        let mut spec = AppBundleSpec::new(executable, &icon, root.join("dist"));
        spec.signing_identity = SigningIdentity::Unsigned;
        let bundle = build_app_bundle(&spec).expect("build app bundle");

        fs::remove_file(&bundle.icon_path).expect("remove icon");
        let err = validate_app_bundle(&bundle).expect_err("missing icon fails");
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn rejects_oversized_info_plist_without_unbounded_read() {
        let root = temp_root("oversized-plist");
        let executable = std::env::current_exe().expect("current test executable");
        let icon = root.join("GFM.icns");
        fs::create_dir_all(&root).expect("create temp root");
        fs::write(&icon, b"icns-test").expect("write icon");
        let mut spec = AppBundleSpec::new(executable, &icon, root.join("dist"));
        spec.signing_identity = SigningIdentity::Unsigned;
        let bundle = build_app_bundle(&spec).expect("build app bundle");
        fs::write(
            &bundle.info_plist_path,
            vec![b'x'; MAX_INFO_PLIST_BYTES as usize + 1],
        )
        .expect("write oversized plist");

        let err = validate_app_bundle(&bundle).expect_err("oversized plist fails");

        assert!(err.to_string().contains("bounded validation budget"));
    }

    #[test]
    fn refuses_unprobeable_bundle_output_before_creating_default_layout() {
        let root = temp_root("bundle-output-probe");
        fs::create_dir_all(&root).expect("create temp root");
        let icon = root.join("GFM.icns");
        fs::write(&icon, b"icns-test").expect("write icon");
        let executable = std::env::current_exe().expect("current test executable");
        let output = root.join("bundle-output-unavailable".repeat(16));
        let mut spec = AppBundleSpec::new(executable, icon, output);
        spec.signing_identity = SigningIdentity::Unsigned;

        let err = build_app_bundle(&spec).expect_err("unprobeable output fails");

        assert!(err
            .to_string()
            .contains("bundle directory probe unavailable"));
        assert!(err.to_string().contains("bundle-output-unavailable"));
        fs::remove_dir_all(root).unwrap();
    }

    fn temp_root(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        std::env::temp_dir().join(format!("gfm-packaging-{name}-{nonce}"))
    }
}
