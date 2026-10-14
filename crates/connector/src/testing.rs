//! The inputs of a supervisor and the status channels of a connector, for the tests
//! of `connector` and of the kinds.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use env::files::{Files, Mode};
use env::net::Net;
use spec::channel::{Channel, Data, Kind};
use spec::data_type::DataType;
use spec::definition::Definition;
use types::channel;
use types::name::Name;

use crate::kind::Table;
use crate::{status, supervisor};

/// The directory of the files that holds the secrets of [`create_secret_store`].
const SECRETS: &str = "secrets";

/// A supervisor's inputs for the tests of `connector` and the kinds: `kinds`, the
/// seams of `env` and `net`, a hub on a new shard in `env`, and the store of
/// [`create_secret_store`] on `env.files`.
///
/// # Panics
///
/// As [`hub::testing::open`].
pub async fn create_config(
    env: hub::testing::Env,
    net: Net,
    kinds: Table,
) -> supervisor::Config {
    let (clock, entropy, tasks) =
        (env.clock.clone(), env.entropy.clone(), env.tasks.clone());
    let files = env.files.clone();
    let (hub, _) = hub::testing::open(env).await;
    supervisor::Config {
        kinds: Arc::new(kinds),
        clock,
        entropy,
        net,
        tasks,
        hub,
        secrets: create_secret_store(files),
    }
}

/// A store of the secrets on `files` that [`create_secret`] writes.
#[must_use]
pub fn create_secret_store(files: Files) -> Box<dyn secret::store::Store> {
    Box::new(secret::store::File::new(files, Path::new(SECRETS)))
}

/// Writes `value` as the secret `name` in the store that [`create_secret_store`]
/// makes on `files`, in place of a value that it had. A read of the store meanwhile
/// gives the old value, no value, or `value`, never a part of one.
///
/// # Panics
///
/// When a call of `files` fails, or another call for `name` runs at once.
pub async fn create_secret(files: &Files, name: &Name, value: &[u8]) {
    let dir = Path::new(SECRETS);
    files.create_dir(dir).await.expect("makes the directory");
    // No name starts with `.`, so the store never reads the staged file. The hash
    // keeps the staged name within 255 bytes for a name of `Name::MAX_BYTES`.
    let mut hasher = DefaultHasher::new();
    name.as_str().hash(&mut hasher);
    let staged = dir.join(format!(".{:016x}", hasher.finish()));
    files
        .remove(&staged)
        .await
        .expect("removes a dropped value");
    let len = u64::try_from(value.len()).expect("invariant: usize fits u64");
    let file = files.open(&staged, Mode::Create { len }).await;
    let mut file = file.expect("makes the file");
    let pool = block::Config::new(1 << 18).expect("a budget that fits");
    let block = block::Pool::heap(pool).copy(value);
    let block = block.expect("the pool holds the value");
    file.write_at(0, &[block]).await.expect("writes the value");
    let path = dir.join(name.as_str());
    files.remove(&path).await.expect("removes the old value");
    file.rename(&path).await.expect("puts the value in place");
    file.close().await;
}

/// The definitions of the status channels of `connector`, whose kind names `counts`,
/// with keys from `first` on, the index first. A test passes them to
/// [`hub::Hub::set_definitions`] with its own.
///
/// # Panics
///
/// When [`status::channels`] gives an error.
#[must_use]
pub fn create_status(
    connector: &Name,
    counts: &[Name],
    first: channel::Key,
) -> Vec<(Name, Definition)> {
    let names = status::channels(connector, counts);
    let (time, channels) = names.expect("the status names are names");
    let index = first;
    let kind = Kind::Index {
        error: None,
        control: None,
    };
    let time = (time, Definition::Channel(Channel { key: index, kind }));
    let channels =
        (first.as_u128() + 1..)
            .zip(channels)
            .map(|(key, (name, sample))| {
                let data = Data::new(index, None, DataType::Sample(sample), None);
                let data = data.expect("invariant: a status channel has no unit");
                let key = channel::Key::from_u128(key);
                let kind = Kind::Data(data);
                (name, Definition::Channel(Channel { key, kind }))
            });
    std::iter::once(time).chain(channels).collect()
}
