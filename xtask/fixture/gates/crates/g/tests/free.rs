fn main() {}

#[cfg(unix)]
fn case() {}

struct Rig;

impl Rig {
    fn case() {}
    #[cfg(unix)]
    fn gated() {}
}

trait Case {
    #[cfg(unix)]
    fn case();
}

#[cfg(unix)]
impl Case for [u8] {
    fn case() {}
}

impl std::fmt::Display for Rig {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

#[cfg(unix)]
impl std::fmt::Debug for Rig {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

m! {
    #[cfg(unix)]
    const fn konst() {}
    #[cfg(unix)]
    unsafe fn risky() {}
    #[cfg(unix)]
    extern "C" fn foreign() {}
}

#[cfg(unix)]
impl Case for &mut Rig {
    fn case() {}
}

#[cfg(unix)]
impl Case for [u8; 2] {
    fn case() {}
}

#[cfg(unix)]
impl Case for (u8, Rig) {
    fn case() {}
}

#[cfg(unix)]
impl Case for <Rig as Case>::Out {
    fn case() {}
}

#[cfg(unix)]
impl Case for Cell<'static, Rig> {
    fn case() {}
}

impl Case for [u8; 3] {
    fn case() {}
}

trait Other {
    fn other();
}

#[cfg(unix)]
impl Other for fn() {
    fn other() {}
}

impl Other for dyn Fn() {
    fn other() {}
}

m! {
    #[cfg(unix)]
    impl Rig {
        fn implied() {}
    }
    #[cfg(unix)]
    static S: u8 = 0;
    impl Rig {
        fn unleaked() {}
    }
}
