use crate::access::{preflight_access_scope, ScopedAccessGuard};
use gfm_mac::AccessIntent;
use gfm_packaging::{
    build_app_bundle, notarize_app_bundle, register_launch_services, require_codesign_toolchain,
    require_release_xcode_toolchain, validate_release_artifact, AppBundleSpec,
    AppleToolchainReport, NotarizationCredentials, NotarizationSpec, ReleaseArtifactSpec,
    ReleasePolicy, SigningIdentity,
};
use gfm_types::{GfmError, Result};
use std::path::{Path, PathBuf};

pub fn release_policy() -> Result<()> {
    let policy = ReleasePolicy::default();
    policy.validate()?;
    print_release_policy(&policy);
    Ok(())
}

pub fn release_validate(args: &mut impl Iterator<Item = String>) -> Result<()> {
    let app_path = required_path(args.next(), "release-validate requires a .app path")?;
    let _access = retain_packaging_read_access(&app_path, "release validate app")?;
    let mut spec = ReleaseArtifactSpec::new(app_path);
    for arg in args {
        match arg.as_str() {
            "--allow-unsigned" => spec.require_signed = false,
            "--skip-notarization" => spec.require_notarized = false,
            "--skip-gatekeeper" => spec.assess_gatekeeper = false,
            other => {
                return Err(GfmError::Format(format!(
                    "unknown release-validate flag {other}"
                )))
            }
        }
    }
    if spec.require_signed || spec.require_notarized || spec.assess_gatekeeper {
        require_release_xcode_toolchain()?;
    }
    let report = validate_release_artifact(&spec)?;
    println!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        report.app_path.display(),
        report.bundle_identifier,
        report.executable_path.display(),
        report.executable_bytes,
        report.signature.as_str(),
        report.notarization.as_str(),
        report.gatekeeper.as_str()
    );
    Ok(())
}

pub fn bundle_app(args: &mut impl Iterator<Item = String>) -> Result<()> {
    let executable = required_path(args.next(), "bundle-app requires an executable path")?;
    let icon = required_path(args.next(), "bundle-app requires an icon path")?;
    let output_dir = required_path(args.next(), "bundle-app requires an output directory")?;
    let _access = retain_bundle_access(&executable, &icon, &output_dir)?;
    let mut spec = AppBundleSpec::new(executable, icon, output_dir);
    spec.signing_identity = match args.next().as_deref() {
        Some("--unsigned") => SigningIdentity::Unsigned,
        Some("--ad-hoc") | None => SigningIdentity::AdHoc,
        Some(identity) => SigningIdentity::DeveloperId(identity.to_string()),
    };
    if spec.signing_identity != SigningIdentity::Unsigned {
        require_codesign_toolchain()?;
    }
    let bundle = build_app_bundle(&spec)?;
    println!(
        "{}\t{}\t{}",
        bundle.app_path.display(),
        bundle.executable_path.display(),
        bundle.signed
    );
    Ok(())
}

pub fn register_app(args: &mut impl Iterator<Item = String>) -> Result<()> {
    let app_path = required_path(args.next(), "register-app requires an .app path")?;
    let _access = preflight_access_scope(&app_path, AccessIntent::Operate, "register app")?;
    register_launch_services(&app_path)?;
    println!("{}", app_path.display());
    Ok(())
}

pub fn notarize_app(args: &mut impl Iterator<Item = String>) -> Result<()> {
    let app_path = required_path(args.next(), "notarize-app requires an .app path")?;
    let output_dir = required_path(args.next(), "notarize-app requires an output directory")?;
    let credentials = notarization_credentials(args)?;
    let _access = retain_notarize_access(&app_path, &output_dir, &credentials)?;
    require_release_xcode_toolchain()?;
    let ticket = notarize_app_bundle(&NotarizationSpec::new(app_path, output_dir, credentials))?;
    println!(
        "{}\t{}\t{:?}\t{}",
        ticket.submission_id,
        ticket.archive_path.display(),
        ticket.status,
        ticket.stapled
    );
    Ok(())
}

pub fn release_toolchain() -> Result<()> {
    print_toolchain_report(&require_release_xcode_toolchain()?);
    Ok(())
}

fn print_release_policy(policy: &ReleasePolicy) {
    println!("channel\t{}", policy.channel.as_str());
    println!(
        "updates\t{}\tfeed={}\tinterval={}\trollout={}\trequire-notarized={}",
        policy.updates.mode.as_str(),
        policy.updates.feed_url.as_deref().unwrap_or("-"),
        policy.updates.minimum_interval_secs,
        policy.updates.staged_rollout_percent,
        policy.updates.require_notarized
    );
    println!(
        "rollback\tenabled={}\tretained={}\trequire-signed={}\tpreserve-user-state={}",
        policy.rollback.enabled,
        policy.rollback.retained_versions,
        policy.rollback.require_signed_previous,
        policy.rollback.preserve_user_state
    );
    println!(
        "crash-reports\t{}\tremote-allowed={}\tretention-days={}\tinclude-minidump={}",
        policy.crash_reports.mode.as_str(),
        policy.remote_crash_upload_allowed(),
        policy.crash_reports.retention_days,
        policy.crash_reports.include_minidump
    );
    println!(
        "diagnostics\t{}\tremote-allowed={}\tretention-days={}",
        policy.diagnostics.mode.as_str(),
        policy.remote_diagnostics_upload_allowed(),
        policy.diagnostics.retention_days
    );
}

fn print_toolchain_report(report: &AppleToolchainReport) {
    println!("developer-dir\t{}", report.developer_dir.display());
    for utility in &report.utilities {
        println!("tool\t{}\t{}", utility.name, utility.path.display());
    }
    println!("metal-smoke-test\t{}", report.metal_smoke_tested);
}

fn retain_packaging_read_access(path: &Path, worker: &str) -> Result<ScopedAccessGuard> {
    preflight_access_scope(path, AccessIntent::Read, worker)
}

fn retain_bundle_access(
    executable: &Path,
    icon: &Path,
    output_dir: &Path,
) -> Result<Vec<ScopedAccessGuard>> {
    Ok(vec![
        retain_packaging_read_access(executable, "bundle app executable")?,
        retain_packaging_read_access(icon, "bundle app icon")?,
        preflight_access_scope(
            write_probe_path(output_dir),
            AccessIntent::Write,
            "bundle app output",
        )?,
    ])
}

fn retain_notarize_access(
    app_path: &Path,
    output_dir: &Path,
    credentials: &NotarizationCredentials,
) -> Result<Vec<ScopedAccessGuard>> {
    let mut guards = vec![
        retain_packaging_read_access(app_path, "notarize app")?,
        preflight_access_scope(
            write_probe_path(output_dir),
            AccessIntent::Write,
            "notarize output",
        )?,
    ];
    if let NotarizationCredentials::ApiKey { key_path, .. } = credentials {
        guards.push(retain_packaging_read_access(key_path, "notarize api key")?);
    }
    Ok(guards)
}

fn write_probe_path(path: &Path) -> &Path {
    if path.is_dir() {
        return path;
    }
    path.parent().unwrap_or(path)
}

fn notarization_credentials(
    args: &mut impl Iterator<Item = String>,
) -> Result<NotarizationCredentials> {
    match args.next().as_deref() {
        Some("--keychain-profile") => args
            .next()
            .map(NotarizationCredentials::KeychainProfile)
            .ok_or_else(|| {
                GfmError::Format(
                    "notarize-app --keychain-profile requires a profile name".to_string(),
                )
            }),
        Some("--apple-id") => {
            let apple_id = args.next().ok_or_else(|| {
                GfmError::Format("notarize-app --apple-id requires an Apple ID".to_string())
            })?;
            require_flag(args.next(), "--team-id", "notarize-app --apple-id")?;
            let team_id = args.next().ok_or_else(|| {
                GfmError::Format("notarize-app --team-id requires a team ID".to_string())
            })?;
            require_flag(args.next(), "--password", "notarize-app --apple-id")?;
            let password = args.next().ok_or_else(|| {
                GfmError::Format(
                    "notarize-app --password requires an app-specific password".to_string(),
                )
            })?;
            Ok(NotarizationCredentials::AppleId {
                apple_id,
                team_id,
                password,
            })
        }
        Some("--api-key") => {
            let key_path = required_path(
                args.next(),
                "notarize-app --api-key requires a .p8 key path",
            )?;
            require_flag(args.next(), "--key-id", "notarize-app --api-key")?;
            let key_id = args.next().ok_or_else(|| {
                GfmError::Format("notarize-app --key-id requires a key ID".to_string())
            })?;
            require_flag(args.next(), "--issuer", "notarize-app --api-key")?;
            let issuer_id = args.next().ok_or_else(|| {
                GfmError::Format("notarize-app --issuer requires an issuer ID".to_string())
            })?;
            Ok(NotarizationCredentials::ApiKey {
                key_id,
                issuer_id,
                key_path,
            })
        }
        Some(other) => Err(GfmError::Format(format!(
            "unknown notarize-app credential flag {other}"
        ))),
        None => Err(GfmError::Format(
            "notarize-app requires --keychain-profile, --apple-id, or --api-key credentials"
                .to_string(),
        )),
    }
}

fn require_flag(value: Option<String>, expected: &str, context: &str) -> Result<()> {
    match value {
        Some(flag) if flag == expected => Ok(()),
        Some(flag) => Err(GfmError::Format(format!(
            "expected {expected} for {context}, got {flag}"
        ))),
        None => Err(GfmError::Format(format!("{context} requires {expected}"))),
    }
}

fn required_path(value: Option<String>, message: &str) -> Result<PathBuf> {
    value
        .map(PathBuf::from)
        .ok_or_else(|| GfmError::Format(message.to_string()))
}
