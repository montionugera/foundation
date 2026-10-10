//! The failed file call at the root of an error of a lower crate. Each match names
//! each variant, so a new variant that wraps a file call cannot go unread.

use env::files;
use mesh::{Stopped, log};

/// The failed file call that caused `error`, or `None`.
pub(crate) fn buffer(error: &buffer::Error) -> Option<&files::Error> {
    match error {
        buffer::Error::Files(error) => Some(error),
        buffer::Error::Full { .. }
        | buffer::Error::Pool(_)
        | buffer::Error::Length { .. }
        | buffer::Error::Missing
        | buffer::Error::Damaged
        | buffer::Error::Version(_)
        | buffer::Error::Unfit(_)
        | buffer::Error::Unaligned { .. }
        | buffer::Error::Invalid { .. } => None,
    }
}

/// The failed file call that caused `error`, or `None`.
pub(crate) fn blob(error: &blob::Error) -> Option<&files::Error> {
    match error {
        blob::Error::Files(error) => Some(error),
        blob::Error::Pool(_)
        | blob::Error::Mismatch { .. }
        | blob::Error::Stray { .. } => None,
    }
}

/// The failed file call that caused `error`, or `None`.
pub(crate) fn mesh(error: &mesh::Error) -> Option<&files::Error> {
    match error {
        mesh::Error::Files(error) => Some(error),
        mesh::Error::Log(error) => self::log(error),
        mesh::Error::Blob(error) => blob(error),
        mesh::Error::Stopped(stopped) => self::stopped(stopped),
        mesh::Error::Raft(_)
        | mesh::Error::Spoofed { .. }
        | mesh::Error::NotVoter { .. }
        | mesh::Error::Removed { .. }
        | mesh::Error::PeerNotVoter { .. }
        | mesh::Error::Claim(_)
        | mesh::Error::NotMember(_)
        | mesh::Error::NoVote
        | mesh::Error::Member(_)
        | mesh::Error::WrongKey
        | mesh::Error::Pool(_)
        | mesh::Error::Malformed
        | mesh::Error::Stream(_)
        | mesh::Error::Stale { .. }
        | mesh::Error::Large { .. }
        | mesh::Error::Problems(_)
        | mesh::Error::Quorum { .. }
        | mesh::Error::Stray { .. }
        | mesh::Error::NotIndex(_)
        | mesh::Error::UnknownNode(_)
        | mesh::Error::Homes { .. }
        | mesh::Error::Founding { .. }
        | mesh::Error::Unfounded { .. } => None,
    }
}

/// The failed file call that stopped the group, or `None`.
pub(crate) fn stopped(stopped: &Stopped) -> Option<&files::Error> {
    match stopped {
        Stopped::Write(error) => log(error),
        Stopped::Change { .. } | Stopped::Removed { .. } | Stopped::Dropped => None,
    }
}

/// The failed file call that caused `error`, or `None`.
fn log(error: &log::Error) -> Option<&files::Error> {
    match error {
        log::Error::Files(error) => Some(error),
        log::Error::Pool(_)
        | log::Error::Corrupt { .. }
        | log::Error::Version { .. }
        | log::Error::Stray { .. }
        | log::Error::Poisoned { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use env::files;
    use mesh::{Stopped, log};

    use crate::Error;

    #[test]
    fn files_gives_the_failed_file_call_of_each_path() {
        let failed = files::Error::Io {
            path: PathBuf::from("mesh/log/log-0"),
            operation: files::Operation::Open,
            code: 13,
        };
        let write = || Stopped::Write(log::Error::Files(failed.clone()));
        for error in [
            Error::Directory(failed.clone()),
            Error::Buffer {
                core: 1,
                error: buffer::Error::Files(failed.clone()),
            },
            Error::Blob(blob::Error::Files(failed.clone())),
            Error::Mesh(mesh::Error::Files(failed.clone())),
            Error::Mesh(mesh::Error::Log(log::Error::Files(failed.clone()))),
            Error::Mesh(mesh::Error::Blob(blob::Error::Files(failed.clone()))),
            Error::Mesh(mesh::Error::Stopped(write())),
            Error::Group(write()),
        ] {
            assert_eq!(error.files(), Some(&failed), "{error:?}");
        }
    }

    #[test]
    fn files_gives_none_when_no_file_call_caused_the_error() {
        let corrupt = log::Error::Corrupt {
            path: PathBuf::from("mesh/log/log-0"),
            offset: 64,
        };
        for error in [
            Error::Mesh(mesh::Error::Log(corrupt)),
            Error::Group(Stopped::Dropped),
            Error::Key,
        ] {
            assert_eq!(error.files(), None, "{error:?}");
        }
    }
}
