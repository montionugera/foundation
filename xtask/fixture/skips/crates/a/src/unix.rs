#[cfg_attr(target_os = "macos", ignore = "only Linux: macOS has no X")]
fn a() {}
#[cfg_attr(target_os = "macos", ignore = "only Linux and Windows: macOS has no X")]
fn b() {}
