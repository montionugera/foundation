#[cfg_attr(not(target_os = "linux"), ignore = "only Linux or macOS: a case")]
fn or() {}
#[cfg_attr(not(target_os = "linux"), ignore = "only Linux, macOS: a case")]
fn comma() {}
#[cfg_attr(not(target_os = "linux"), ignore = "only Linux/macOS: a case")]
fn slash() {}
