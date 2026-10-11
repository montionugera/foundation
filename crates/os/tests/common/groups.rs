//! The groups of the user of the test.

/// A group of the user that is not `made`, the group of a new file.
///
/// # Panics
///
/// When the user is in one group only.
pub(crate) fn other(made: u32) -> u32 {
    let groups = rustix::process::getgroups().unwrap();
    let mut groups = groups.iter().map(|group| group.as_raw());
    let other = groups.find(|group| *group != made);
    other.expect("the user of the test is in two groups")
}
