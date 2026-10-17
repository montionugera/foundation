#[cfg_attr(not(target_os = "linux"), ignore = "only Linux: seccomp")]
fn a() {}
#[cfg_attr(miri, ignore = "Miri cannot spawn a process")]
#[cfg_attr(windows, expect(unused))]
fn b() {}
#[cfg_attr(feature = "unix", ignore = "a feature")]
fn c() {}
#[cfg_attr(any(unix, windows), ignore = "slow")]
fn d() {}
#[cfg_attr(not(unix), ignore = r#"only Unix: a "case""#)]
fn e() {}
#[cfg_attr(windows, ignore = "only Unix: by a (case), with commas")]
fn f() {}
#[cfg_attr(not(target_os = "macos"), ignore = "only macOS")]
fn g() {}
#[cfg_attr(all(target_os = "linux", miri), ignore = "Miri on Linux")]
fn h() {}
#[cfg_attr(target_env = "msvc", ignore = "only Unix: a case")]
fn i() {}
#[cfg_attr(not(target_vendor = "apple"), ignore = "only macOS: a case")]
fn j() {}
#[cfg_attr(all(windows, test), ignore = "only Unix: a case")]
// #[cfg_attr(windows, ignore)]
fn k() {
    let _ = "#[cfg_attr(windows, ignore)]";
}
#[cfg_attr(target_os = "macos", ignore = "only Linux: macOS has no X")]
#[cfg_attr(windows, ignore = "only Linux: Windows has no Y")]
fn l() {}
#[cfg_attr(target_os = "macos", ignore = "only Linux and Windows: macOS has no X")]
fn m() {}
#[cfg_attr(not(target_env = "gnu"), ignore = "only Linux: glibc")]
fn n() {}
#[cfg_attr(all(target_os = "linux", target_arch = "aarch64"), ignore = "slow")]
fn o() {}
#[cfg(unix)]
#[cfg_attr(target_os = "macos", ignore = "only Linux: macOS has no X")]
fn p() {}
#[cfg_attr(loom, ignore)]
fn q() {}
#[ignore = "waits on #1"]
#[cfg_attr(windows, ignore = "needs a tool")]
fn r() {}
#[cfg_attr(test, ignore)]
fn s() {}
