mod arity;
mod list;
mod literal;
mod missing;
#[path = "../../../outside.rs"]
mod outside;
mod parse;
mod passed;
mod pathed;
mod refused;
mod target;
#[cfg(unix)]
mod unix;
mod unknown;
#[cfg(windows)]
mod windows;

#[test]
#[cfg_attr(windows, ignore = "only Unix: a case")]
#[cfg_attr(windows, ignore = "needs a tool")]
fn a() {}
