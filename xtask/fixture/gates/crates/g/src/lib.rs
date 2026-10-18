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

/// Only aarch64: a case.
#[cfg(target_os = "macos")]
#[test]
fn apple() {}

/// Only x86_64: a case.
#[cfg(not(target_os = "macos"))]
#[test]
fn intel() {}

/// Only : a case.
#[cfg(all(unix, windows))]
#[test]
fn colon() {}

cfg_if::cfg_if! {
    if #[cfg(unix)] {
        #[test]
        fn hidden() {}
    } else if #[cfg(target_os = "linux")] {
        #[test]
        fn shadowed() {}
    } else {
        #[test]
        fn rest() {}
    }
}

#[cfg(unix)]
m! {
    {
        #[test]
        fn grouped() {}
    }
}

#[cfg(unix)]
m! {
    #![proptest_config(c)]
    const unsafe X: u8 = 1;
    #[a] y;
    pub z;
    mod declared;
    struct S {}
    #[test]
    fn tail() {}
    pub
}

m! {
    mod inside {
        #![cfg(unix)]
        #[test]
        fn kept() {}
    }
}

/// Only aarch64
#[cfg(target_arch = "aarch64")]
#[test]
fn bare() {}

#[cfg_attr(target_os = "freebsd", test)]
fn unbuilt() {}

#[cfg_attr(miri, test)]
fn flagged_test() {}

m! {
    if #[cfg(windows)] {}
    {
        #[test]
        fn after() {}
    }
}

cfg_if::cfg_if! {
    if #[doc(hidden)] {}
}
/// Only macOS: a case.
#[cfg(target_arch = "aarch64")]
#[test]
fn falsemac() {}
