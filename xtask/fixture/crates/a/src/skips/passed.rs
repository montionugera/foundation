#[cfg_attr(not(target_os = "linux"), ignore = "only Linux: seccomp")]
#[cfg_attr(miri, ignore = "Miri cannot spawn a process")]
#[cfg_attr(windows, expect(unused))]
#[cfg_attr(feature = "unix", ignore = "a feature")]
#[cfg_attr(any(unix, windows), ignore = "slow")]
#[cfg_attr(not(unix), ignore = r#"only Unix: a "case""#)]
#[cfg_attr(windows, ignore = "only Unix, by a (case), with commas")]
#[cfg_attr(not(target_os = "macos"), ignore = "only macOS")]
#[cfg_attr(all(target_os = "linux", miri), ignore = "Miri on Linux")]
#[cfg_attr(target_env = "msvc", ignore = "only Unix: a case")]
#[cfg_attr(not(target_vendor = "apple"), ignore = "only macOS: a case")]
#[cfg_attr(all(windows, test), ignore = "only Unix: a case")]
// #[cfg_attr(windows, ignore)]
fn a() {
    let _ = "#[cfg_attr(windows, ignore)]";
}
