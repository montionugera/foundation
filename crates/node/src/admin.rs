//! The private key of the first admin of the root region, in the file
//! [`sector::admin`] of the data directory.

use std::path::Path;

use env::files::Files;
use sector::Held;
use sector::admin::{FILE, encode};
use types::ed25519::PrivateKey;

use crate::Error;

/// The private key in `admin.key` of `files`. When the file is not there, makes a new
/// one from `entropy` and makes it durable. Call it under the lock of the data
/// directory, after the sync of the directory that the claim makes: a failed sync of
/// an earlier start can leave a key that a read sees but a crash loses.
///
/// # Errors
///
/// [`Error::Admin`] for a file that a node did not write, and [`Error::Directory`] for
/// a file call that fails.
pub(crate) async fn load(
    files: &Files,
    entropy: &env::entropy::Entropy,
) -> Result<PrivateKey, Error> {
    match sector::admin::read(files).await.map_err(Error::Directory)? {
        Held::Nothing => {
            let mut bytes = [0; 32];
            entropy.fill(&mut bytes);
            let private_key = PrivateKey(bytes);
            sector::publish(files, Path::new(FILE), &encode(&private_key))
                .await
                .map_err(Error::Directory)?;
            Ok(private_key)
        }
        Held::Written(private_key) => Ok(private_key),
        Held::Version { .. } | Held::Foreign => Err(Error::Admin),
    }
}
