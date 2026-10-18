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
