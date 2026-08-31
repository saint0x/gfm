use crate::access::{
    preflight_access_scope_checked_with_volume_report, preflight_volume_access_scope_with_report,
    ScopedAccessGuard,
};
use crate::runtime::run_volume_task_cancellable;
use gfm_jobs::Priority;
use gfm_mac::{AccessIntent, VolumeDiscoveryReport};
use gfm_packaging::{
    build_app_bundle, notarize_app_bundle, register_launch_services, require_codesign_toolchain,
    require_release_xcode_toolchain, validate_release_artifact, AppBundle, AppBundleSpec,
    AppleToolchainReport, NotarizationCredentials, NotarizationSpec, NotarizationTicket,
    ReleaseArtifactReport, ReleaseArtifactSpec, ReleasePolicy, SigningIdentity,
};
use gfm_types::{GfmError, Result, VolumeId};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub fn release_policy() -> Result<()> {
    let policy = ReleasePolicy::default();
    policy.validate()?;
    print_release_policy(&policy);
    Ok(())
}

pub fn release_validate(args: &mut impl Iterator<Item = String>) -> Result<()> {
    let app_path = required_path(args.next(), "release-validate requires a .app path")?;
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
    let report = run_release_validate(spec)?;
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
    let mut spec = AppBundleSpec::new(executable, icon, output_dir);
    spec.signing_identity = match args.next().as_deref() {
        Some("--unsigned") => SigningIdentity::Unsigned,
        Some("--ad-hoc") | None => SigningIdentity::AdHoc,
        Some(identity) => SigningIdentity::DeveloperId(identity.to_string()),
    };
    let bundle = run_bundle_app(spec)?;
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
    run_register_app(app_path.clone())?;
    println!("{}", app_path.display());
    Ok(())
}

pub fn notarize_app(args: &mut impl Iterator<Item = String>) -> Result<()> {
    let app_path = required_path(args.next(), "notarize-app requires an .app path")?;
    let output_dir = required_path(args.next(), "notarize-app requires an output directory")?;
    let credentials = notarization_credentials(args)?;
    let ticket = run_notarize_app(NotarizationSpec::new(app_path, output_dir, credentials))?;
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

fn run_release_validate(spec: ReleaseArtifactSpec) -> Result<ReleaseArtifactReport> {
    const WORKER: &str = "release validate app";
    let access_report = PackagingAccessReport::new_checked(
        spec.app_path.clone(),
        AccessIntent::Read,
        WORKER,
        || Ok(()),
    )?;
    access_report.preflight_volume()?;
    let volume = access_report.volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        if spec.require_signed || spec.require_notarized || spec.assess_gatekeeper {
            require_release_xcode_toolchain()?;
        }
        validate_release_artifact(&spec)
    })
}

fn run_bundle_app(spec: AppBundleSpec) -> Result<AppBundle> {
    const WORKER: &str = "bundle app";
    let access_reports = PackagingAccessReports::bundle(&spec)?;
    access_reports.preflight_volumes()?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        if spec.signing_identity != SigningIdentity::Unsigned {
            require_codesign_toolchain()?;
        }
        build_app_bundle(&spec)
    })
}

fn run_register_app(app_path: PathBuf) -> Result<()> {
    const WORKER: &str = "register app";
    let access_report = PackagingAccessReport::new_checked(
        app_path.clone(),
        AccessIntent::Operate,
        WORKER,
        || Ok(()),
    )?;
    access_report.preflight_volume()?;
    let volume = access_report.volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_report.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        register_launch_services(&app_path)
    })
}

fn run_notarize_app(spec: NotarizationSpec) -> Result<NotarizationTicket> {
    const WORKER: &str = "notarize app";
    let access_reports = PackagingAccessReports::notarize(&spec)?;
    access_reports.preflight_volumes()?;
    let volume = access_reports.first_volume();
    run_volume_task_cancellable(volume, Priority::Visible, WORKER, move |cancellation| {
        cancellation.check()?;
        let _access = access_reports.access_checked(|| cancellation.check())?;
        cancellation.check()?;
        require_release_xcode_toolchain()?;
        notarize_app_bundle(&spec)
    })
}

#[derive(Clone)]
struct PackagingAccessReport {
    path: PathBuf,
    intent: AccessIntent,
    worker: &'static str,
    volume_report: VolumeDiscoveryReport,
}

impl PackagingAccessReport {
    fn new_checked(
        path: PathBuf,
        intent: AccessIntent,
        worker: &'static str,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let volume_report =
            VolumeDiscoveryReport::for_containing_path_checked(&path, &mut check_control)?;
        check_control()?;
        Ok(Self {
            path,
            intent,
            worker,
            volume_report,
        })
    }

    fn preflight_volume(&self) -> Result<()> {
        preflight_volume_access_scope_with_report(
            &self.path,
            self.intent,
            self.worker,
            &self.volume_report,
        )
    }

    fn access_checked(
        &self,
        check_control: impl FnMut() -> Result<()>,
    ) -> Result<ScopedAccessGuard> {
        preflight_access_scope_checked_with_volume_report(
            &self.path,
            self.intent,
            self.worker,
            &self.volume_report,
            check_control,
        )
    }

    fn volume(&self) -> Option<VolumeId> {
        self.volume_report
            .volume_for_path(&self.path)
            .map(|volume| volume.id)
    }
}

#[derive(Clone)]
struct PackagingAccessReports {
    entries: Vec<PackagingAccessReport>,
}

impl PackagingAccessReports {
    fn bundle(spec: &AppBundleSpec) -> Result<Self> {
        Self::bundle_checked(spec, || Ok(()))
    }

    fn bundle_checked(
        spec: &AppBundleSpec,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let output_probe = write_probe_path(&spec.output_dir)?.to_path_buf();
        check_control()?;
        Ok(Self {
            entries: vec![
                PackagingAccessReport::new_checked(
                    spec.executable.clone(),
                    AccessIntent::Read,
                    "bundle app executable",
                    &mut check_control,
                )?,
                PackagingAccessReport::new_checked(
                    spec.icon.clone(),
                    AccessIntent::Read,
                    "bundle app icon",
                    &mut check_control,
                )?,
                PackagingAccessReport::new_checked(
                    output_probe,
                    AccessIntent::Write,
                    "bundle app output",
                    &mut check_control,
                )?,
            ],
        })
    }

    fn notarize(spec: &NotarizationSpec) -> Result<Self> {
        Self::notarize_checked(spec, || Ok(()))
    }

    fn notarize_checked(
        spec: &NotarizationSpec,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        check_control()?;
        let output_probe = write_probe_path(&spec.output_dir)?.to_path_buf();
        check_control()?;
        let mut entries = vec![
            PackagingAccessReport::new_checked(
                spec.app_path.clone(),
                AccessIntent::Read,
                "notarize app",
                &mut check_control,
            )?,
            PackagingAccessReport::new_checked(
                output_probe,
                AccessIntent::Write,
                "notarize output",
                &mut check_control,
            )?,
        ];
        if let NotarizationCredentials::ApiKey { key_path, .. } = &spec.credentials {
            check_control()?;
            entries.push(PackagingAccessReport::new_checked(
                key_path.clone(),
                AccessIntent::Read,
                "notarize api key",
                &mut check_control,
            )?);
        }
        Ok(Self { entries })
    }

    fn preflight_volumes(&self) -> Result<()> {
        for entry in &self.entries {
            entry.preflight_volume()?;
        }
        Ok(())
    }

    fn access_checked(
        &self,
        mut check_control: impl FnMut() -> Result<()>,
    ) -> Result<Vec<ScopedAccessGuard>> {
        let mut guards = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            check_control()?;
            guards.push(entry.access_checked(&mut check_control)?);
        }
        check_control()?;
        Ok(guards)
    }

    fn first_volume(&self) -> Option<VolumeId> {
        self.entries.iter().find_map(PackagingAccessReport::volume)
    }
}

#[cfg(test)]
fn retain_bundle_access_checked(
    executable: &Path,
    icon: &Path,
    output_dir: &Path,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let access_reports = PackagingAccessReports::bundle_checked(
        &AppBundleSpec::new(
            executable.to_path_buf(),
            icon.to_path_buf(),
            output_dir.to_path_buf(),
        ),
        &mut check_control,
    )?;
    access_reports.access_checked(&mut check_control)
}

#[cfg(test)]
fn retain_notarize_access_checked(
    app_path: &Path,
    output_dir: &Path,
    credentials: &NotarizationCredentials,
    mut check_control: impl FnMut() -> Result<()>,
) -> Result<Vec<ScopedAccessGuard>> {
    check_control()?;
    let access_reports = PackagingAccessReports::notarize_checked(
        &NotarizationSpec::new(
            app_path.to_path_buf(),
            output_dir.to_path_buf(),
            credentials.clone(),
        ),
        &mut check_control,
    )?;
    access_reports.access_checked(&mut check_control)
}

fn write_probe_path(path: &Path) -> Result<&Path> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(path),
        Ok(_) => Ok(crate::parent_or_cwd(path)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(crate::parent_or_cwd(path)),
        Err(err) => Err(GfmError::io(
            path,
            format!("packaging write path metadata unavailable: {err}"),
        )),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn packaging_access_report_checked_honors_pre_cancelled_control_before_volume_discovery() {
        let path = std::env::temp_dir()
            .join(format!(
                "gfm-packaging-report-pre-cancel-{}",
                std::process::id()
            ))
            .join("GFM.app");

        let result = PackagingAccessReport::new_checked(
            path.clone(),
            AccessIntent::Read,
            "release validate",
            || Err(GfmError::Cancelled),
        );

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!path.exists());
    }

    #[test]
    fn bundle_access_checked_honors_pre_cancelled_control() {
        let root = unique_temp_dir("gfm-bundle-access-pre-cancel");
        let executable = root.join("gfm");
        let icon = root.join("gfm.icns");
        let output = root.join("GFM.app");

        let result =
            retain_bundle_access_checked(&executable, &icon, &output, || Err(GfmError::Cancelled));

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(!output.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn notarize_access_checked_can_cancel_before_api_key_probe() {
        let root = unique_temp_dir("gfm-notarize-access-cancel");
        let app = root.join("GFM.app");
        let output = root.join("notary");
        let key_path = root.join("AuthKey.p8");
        fs::create_dir_all(&app).unwrap();
        fs::write(&key_path, "key").unwrap();
        let credentials = NotarizationCredentials::ApiKey {
            key_id: "KEY".to_string(),
            issuer_id: "ISSUER".to_string(),
            key_path,
        };
        let mut checks = 0usize;

        let result = retain_notarize_access_checked(&app, &output, &credentials, || {
            checks += 1;
            if checks >= 5 {
                Err(GfmError::Cancelled)
            } else {
                Ok(())
            }
        });

        assert_eq!(result.err(), Some(GfmError::Cancelled));
        assert!(checks >= 5);
        assert!(!output.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
