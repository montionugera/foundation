pub fn ready() -> bool {
    true
}

#[cfg_attr(target_os = "macos", ignore)]
#[test]
fn ready_on_each_os() {
    assert!(ready());
}
