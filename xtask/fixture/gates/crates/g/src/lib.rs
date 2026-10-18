mod module;
mod twins;

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
    let a = [0];
    #[test]
    fn generated() {}
    #[inline]
    fn helper() {}
}

#[cfg(unix)]
m! {
    #[test]
    pub fn public() {}
    #[tokio::test]
    async fn waited() {}
    mod inner {
        #[test]
        fn nested() {}
    }
}

#[cfg(miri)]
#[test]
fn flagged() {}

/// Only sparc64: a case.
#[cfg(target_os = "freebsd")]
mod nowhere {
    #[test]
    fn bsd() {}
}

#[cfg(unix)]
m! {
    #[a b]
    fn unread() {}
}

/// Only aarch64: a case.
#[cfg(target_arch = "aarch64")]
#[test]
fn armed() {}

/// Only x86_64: a case.
#[cfg(target_arch = "aarch64")]
#[test]
fn misarmed() {}
