use gfm_types::{GfmError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod notarize;
mod policy;

pub use notarize::{
    notarize_app_bundle, NotarizationCredentials, NotarizationSpec, NotarizationStatus,
    NotarizationTicket,
};
pub use policy::{
    CrashReportMode, CrashReportPolicy, DiagnosticMode, DiagnosticsPolicy, ReleaseChannel,
    ReleasePolicy, RollbackPolicy, UpdateDecision, UpdateMode, UpdatePolicy,
};

const DEFAULT_MINIMUM_SYSTEM_VERSION: &str = "14.0";

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
    let plist = read_to_string(&bundle.info_plist_path)?;
    for required in [
        "CFBundleExecutable",
        "CFBundleIdentifier",
        "CFBundleIconFile",
        "CFBundleDocumentTypes",
        "LSItemContentTypes",
        "public.folder",
        "public.item",
        "LSMinimumSystemVersion",
    ] {
        if !plist.contains(required) {
            return Err(GfmError::Format(format!(
                "{} missing required Info.plist key or value `{required}`",
                bundle.info_plist_path.display()
            )));
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
    if path.exists() {
        fs::remove_dir_all(path).map_err(|err| GfmError::io(path, err))?;
    }
    create_dir(path)
}

fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|err| GfmError::io(path, err))
}

fn ensure_dir(path: &Path) -> Result<()> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(GfmError::Format(format!(
            "{} is missing or is not a directory",
            path.display()
        )))
    }
}

fn ensure_file(path: &Path) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(GfmError::Format(format!(
            "{} is missing or is not a file",
            path.display()
        )))
    }
}

fn copy_file(from: &Path, to: &Path) -> Result<()> {
    fs::copy(from, to).map_err(|err| GfmError::io(to, err))?;
    Ok(())
}

fn write_file(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path, contents).map_err(|err| GfmError::io(path, err))
}

fn read_to_string(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|err| GfmError::io(path, err))
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

    fn temp_root(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        std::env::temp_dir().join(format!("gfm-packaging-{name}-{nonce}"))
    }
}
