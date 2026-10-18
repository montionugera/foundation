mod module;

#[cfg(unix)]
#[test]
fn refused() {}

/// Only Unix: a case.
#[cfg(unix)]
#[test]
fn given() {}

/// Only Linux: a case.
#[cfg(unix)]
#[test]
fn misnamed() {}

#[cfg(unix)]
#[test]
fn listed() {}

#[cfg(unix)]
#[test]
fn paired() {}

#[cfg(windows)]
#[test]
fn paired() {}

/// Only Linux: a case.
#[cfg(target_os = "linux")]
#[test]
fn both() {}

#[cfg(unix)]
fn helper() {}

#[cfg(feature = "x")]
#[test]
fn featured() {}

#[cfg(target_os = "macos")]
m! {
    #[test]
    fn generated() {}
}
