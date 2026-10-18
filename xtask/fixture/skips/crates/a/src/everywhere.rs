#[cfg_attr(unix, ignore = "flaky")]
#[cfg_attr(windows, ignore = "fails on Windows")]
fn a() {}

#[cfg_attr(any(unix, windows), ignore = "flaky")]
fn b() {}

#[cfg_attr(not(target_os = "freebsd"), ignore)]
fn c() {}
