use super::{plan_upgrade, UpgradePlan};
use crate::self_update::InstallMethod;

const EXE: &str = "/home/u/.local/bin/deck";
const TRIPLE: &str = "aarch64-apple-darwin";

fn plan(method: InstallMethod, triple: Option<&str>) -> UpgradePlan {
    plan_upgrade(method, "1.3.0", triple, Some(EXE.to_string()))
}

#[test]
fn a_brew_install_upgrades_through_brew() {
    assert_eq!(
        plan(InstallMethod::Brew, Some(TRIPLE)),
        UpgradePlan::Run {
            program: "brew".into(),
            args: vec!["upgrade".into(), "cross-entropy-ai/tap/deck".into()],
        }
    );
}

/// Brew never consults the target triple: the tap knows what it published,
/// and a platform we ship no *binary* for can still have a formula.
#[test]
fn a_brew_install_does_not_care_about_the_target_triple() {
    assert_eq!(
        plan(InstallMethod::Brew, None),
        plan(InstallMethod::Brew, Some(TRIPLE))
    );
}

/// The download route re-execs deck itself, so the version has to reach the
/// child — that argument is how the upgrade knows what to fetch.
#[test]
fn a_writable_install_re_execs_itself_with_the_target_version() {
    assert_eq!(
        plan(InstallMethod::DirectDownload, Some(TRIPLE)),
        UpgradePlan::Run {
            program: EXE.into(),
            args: vec!["__upgrade-self".into(), "1.3.0".into()],
        }
    );
}

/// A process that cannot name its own path still has to run *something*;
/// `deck` off `PATH` is the honest fallback.
#[test]
fn an_unknowable_exe_path_falls_back_to_the_bare_name() {
    let plan = plan_upgrade(InstallMethod::DirectDownload, "1.3.0", Some(TRIPLE), None);
    let UpgradePlan::Run { program, .. } = plan else {
        panic!("a writable install must still plan a run");
    };
    assert_eq!(program, "deck");
}

/// No published binary for this platform: refuse rather than re-exec into a
/// downloader that would find nothing to download.
#[test]
fn a_platform_we_publish_nothing_for_is_refused_not_attempted() {
    let plan = plan(InstallMethod::DirectDownload, None);
    let Some(warning) = plan.warning() else {
        panic!("an unsupported platform must refuse");
    };
    assert_eq!(warning.text, "Unsupported platform");
    assert!(
        warning.detail.contains("cargo install"),
        "the refusal must say how to upgrade anyway: {:?}",
        warning.detail
    );
}

/// An unwritable install location refuses too, and names the version so the
/// user can go get it by hand.
#[test]
fn an_unwritable_install_refuses_and_names_the_version() {
    let plan = plan(InstallMethod::Manual, Some(TRIPLE));
    let Some(warning) = plan.warning() else {
        panic!("an unwritable install must refuse");
    };
    assert_eq!(warning.text, "deck can't self-update from this location");
    assert!(
        warning.detail.contains("1.3.0"),
        "the hint must name the version: {:?}",
        warning.detail
    );
}

/// Only a refusal produces a warning; a plan that runs must not.
#[test]
fn a_runnable_plan_carries_no_warning() {
    assert!(plan(InstallMethod::Brew, Some(TRIPLE)).warning().is_none());
}
