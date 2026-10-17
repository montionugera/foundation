#[cfg_attr(not(target_os = "linux"), ignore = "needs GNU nm")]
#[test]
fn a() {}
#[cfg_attr(windows, ignore)]
fn b() {}
#[cfg_attr(
    not(any(target_os = "linux", target_family = "wasm")),
    ignore = "needs /proc"
)]
fn c() {}
#[cfg_attr(unix, ignore = "Only Windows")]
fn d() {}
#[cfg_attr(target_os = "macos", should_panic, ignore)]
fn e() {}
#[cfg_attr(test, cfg_attr(windows, ignore))]
fn f() {}
#[cfg_attr(windows, cfg_attr(test, ignore))]
fn g() {}
#[cfg_attr(not(target_env = "gnu"), ignore = "the message is glibc's")]
fn h() {}
#[cfg_attr(target_vendor = "apple", ignore = "only Linux")]
fn i() {}
#[cfg_attr(not(target_os = "linux"), ignore = "only when slow")]
fn j() {}
#[cfg_attr(windows, ignore = "only Windows: a case")]
fn k() {}
#[cfg_attr(not(target_os = "linux"), ignore = "only Unix: a case")]
fn l() {}
#[cfg_attr(windows, ignore = "only Unixes")]
fn m() {}
#[cfg_attr(target_family = "windows", ignore)]
fn q() {}
#[cfg_attr(all(feature = "x", windows), ignore)]
fn r() {}
mod n {
    impl X {
        #[cfg_attr(not(windows), ignore)]
        fn o() {}
    }
    trait Y {
        #[cfg_attr(target_os = "macos", ignore)]
        fn p() {}
    }
}
