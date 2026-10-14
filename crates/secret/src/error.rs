//! Why a source of a secret gave no value.

use std::fmt;

/// Why a source gave no value. `Display` gives whole sentences that end with the fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The source holds no value.
    Missing {
        /// The source's own text, which `Display` gives alone: why it holds no value,
        /// and where to put it when it can say, as whole sentences.
        detail: String,
    },
    /// The store cannot hold the name, at any time. Renaming the secret fixes it.
    BadName {
        /// The store's own text: how to rename the secret, and why.
        detail: String,
    },
    /// The store refused this node.
    Denied {
        /// The store's own text.
        detail: String,
    },
    /// The source failed before it gave a value: the value is more than 64 KiB, a
    /// file call failed, or the store did not answer. A get after the fix can
    /// succeed.
    Unavailable {
        /// The source's own text, which `Display` gives alone: what failed and the
        /// fix, as whole sentences.
        detail: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { detail } | Self::Unavailable { detail } => {
                f.write_str(detail)
            }
            Self::BadName { detail } => {
                write!(f, "the store cannot hold the name. {detail}")
            }
            Self::Denied { detail } => write!(
                f,
                "the store refused this node: {detail}. Grant the node access in the \
                 store"
            ),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_the_caller_what_to_do() {
        let cases = [
            (
                Error::Missing {
                    detail: "the file secrets/a is not there. Write the value to it"
                        .into(),
                },
                "the file secrets/a is not there. Write the value to it",
            ),
            (
                Error::BadName {
                    detail: "Rename the secret in lowercase letters".into(),
                },
                "the store cannot hold the name. Rename the secret in lowercase letters",
            ),
            (
                Error::Denied {
                    detail: "403".into(),
                },
                "the store refused this node: 403. Grant the node access in the store",
            ),
            (
                Error::Unavailable {
                    detail: "the store did not answer in 5 s. Get the value again"
                        .into(),
                },
                "the store did not answer in 5 s. Get the value again",
            ),
        ];
        for (error, message) in cases {
            assert_eq!(error.to_string(), message);
        }
    }
}
