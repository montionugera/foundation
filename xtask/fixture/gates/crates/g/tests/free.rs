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
