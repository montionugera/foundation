//! Test helpers for the modules of this crate.

use std::sync::Arc;

use codec::Variable;
use env::clock::Clock;
use env::entropy::Entropy;
use env::tasks::Tasks;
use hub::Hub;
use hub::home::Outcome;
use hub::writer::Writer;
use spec::definition::Definition;
use types::authority::Authority;
use types::channel;
use types::frame::{self, Form, Label, Path};
use types::name::Name;

use crate::kind::Table;
use crate::{supervisor, testing};

/// Runs `main` on a shard of one simulated node and returns its output.
pub(crate) fn run<T, F>(
    main: impl FnOnce(Clock, Tasks, Entropy) -> F + Send + 'static,
) -> T
where
    T: Send + 'static,
    F: Future<Output = T> + 'static,
{
    run_on(|node, tasks| main(node.clock(), tasks, node.entropy()))
}

/// Runs `main` on a shard of one simulated node, given the node, and returns its
/// output.
pub(crate) fn run_on<T, F>(
    main: impl FnOnce(sim::node::Node, Tasks) -> F + Send + 'static,
) -> T
where
    T: Send + 'static,
    F: Future<Output = T> + 'static,
{
    run_in(0, main)
}

/// [`run_on`] in a sim of `seed`.
pub(crate) fn run_in<T, F>(
    seed: u64,
    main: impl FnOnce(sim::node::Node, Tasks) -> F + Send + 'static,
) -> T
where
    T: Send + 'static,
    F: Future<Output = T> + 'static,
{
    run_with(seed, sim::node::Config::default(), main)
}

/// Runs `main` on a shard of one simulated node of `config`, in a sim of `seed`,
/// given the node, and returns its output.
pub(crate) fn run_with<T, F>(
    seed: u64,
    config: sim::node::Config,
    main: impl FnOnce(sim::node::Node, Tasks) -> F + Send + 'static,
) -> T
where
    T: Send + 'static,
    F: Future<Output = T> + 'static,
{
    let mut sim = sim::Sim::new(sim::Config {
        seed,
        ..sim::Config::default()
    });
    let node = sim.node(config);
    sim.run_on(&node, main).expect("the run ends")
}

/// The inputs of a supervisor of `kinds` on a shard of `node`, with a hub on a new
/// shard that defines the status channels of `connector` with keys from
/// [`STATUS`] on.
pub(crate) async fn create_config(
    node: &sim::node::Node,
    tasks: Tasks,
    kinds: Table,
    connector: &str,
) -> supervisor::Config {
    let config = testing::create_config(env(node, tasks), node.net(), kinds).await;
    let status = create_status(connector);
    config
        .hub
        .set_definitions(status.iter().map(|(name, def)| (name, def)));
    config
}

/// The inputs of a hub on a new shard of `node`.
pub(crate) fn env(node: &sim::node::Node, tasks: Tasks) -> hub::testing::Env {
    hub::testing::Env {
        files: node.files(),
        clock: node.clock(),
        wall: node.wall(),
        entropy: node.entropy(),
        tasks,
    }
}

/// The first key of the status channels that [`create_config`] defines.
pub(crate) const STATUS: channel::Key = channel::Key::from_u128(100);

/// The definitions of the status channels of `connector`, with no count, with keys
/// from [`STATUS`] on.
pub(crate) fn create_status(connector: &str) -> Vec<(Name, Definition)> {
    let connector = connector.parse().expect("a valid name");
    testing::create_status(&connector, &[], STATUS)
}

/// The samples of one channel in a frame.
pub(crate) enum Column {
    Numbers(Vec<u64>),
    Texts(Vec<&'static str>),
}

/// Opens a writer of the status channels of `connector` on `hub` as a program, with
/// authority 1: the index `time`, then the channel of each last segment of `lasts`.
pub(crate) async fn open_status(hub: &Hub, connector: &str, lasts: &[&str]) -> Writer {
    let status = |last: &&str| {
        let name = format!("{connector}.status.{last}");
        name.parse().expect("a valid name")
    };
    let config = hub::writer::Config {
        subject: "program".parse().expect("a valid name"),
        authority: Authority(1),
        lease: None,
        channels: ["time"].iter().chain(lasts).map(status).collect(),
    };
    hub.writer(config).await.expect("the program opens")
}

/// Writes one frame through `writer`: `columns` to its channels after the index, in
/// order, and a stamp of the index for each sample, 1 us apart. Returns the outcome
/// of each home.
pub(crate) fn write_columns(writer: &mut Writer, columns: Vec<Column>) -> Vec<Outcome> {
    let entries = writer.entries().to_vec();
    let set = Arc::clone(writer.set());
    let count = match &columns[0] {
        Column::Numbers(values) => values.len(),
        Column::Texts(values) => values.len(),
    };
    let start = u64::try_from(writer.now().nanos()).expect("a stamp after 1970");
    let stamps = (0..count).map(|i| start + u64::try_from(i).expect("few") * 1_000);
    let stamps = Column::Numbers(stamps.collect());
    let columns = [stamps].into_iter().chain(columns);
    let bytes: Vec<(usize, Vec<u8>)> = entries
        .iter()
        .zip(columns)
        .map(|(&entry, column)| {
            let data_type = set.entries()[entry].data_type;
            let bytes = match column {
                Column::Numbers(values) => {
                    let width = data_type.width().expect("a fixed width");
                    let sample = |value: u64| value.to_le_bytes()[..width].to_vec();
                    values.into_iter().flat_map(sample).collect()
                }
                Column::Texts(values) => {
                    let form = Variable::of(data_type).expect("a variable type");
                    let len = form.len(&values).expect("a short text");
                    let mut bytes = vec![0; len];
                    form.write(&values, &mut bytes);
                    bytes
                }
            };
            (entry, bytes)
        })
        .collect();
    let mut series: Vec<_> = bytes
        .iter()
        .map(|(entry, bytes)| (*entry, bytes.len()))
        .collect();
    series.sort_unstable();
    let mut draft = writer.draft(Form::Raw, &series).expect("a draft");
    for (entry, bytes) in &bytes {
        let series = draft.series_mut(*entry).expect("a series of the draft");
        series.copy_from_slice(bytes);
    }
    let group = set.entries()[entries[0]].group;
    draft.set_count(group, u32::try_from(count).expect("few"));
    let outcomes = writer.write(Label::Path(Path::Live), draft);
    outcomes.expect("the write goes").to_vec()
}

/// Writes one frame of `columns`, each by the last segment of its channel, to the
/// status channels of `connector` on `hub`, through a writer of [`open_status`].
pub(crate) async fn write_status(
    hub: &Hub,
    connector: &str,
    columns: Vec<(&'static str, Column)>,
) {
    let lasts: Vec<_> = columns.iter().map(|(last, _)| *last).collect();
    let mut writer = open_status(hub, connector, &lasts).await;
    write_columns(
        &mut writer,
        columns.into_iter().map(|(_, column)| column).collect(),
    );
}

/// Takes frames of one sample for each channel of `writer` from the shard's pool
/// until the pool has no room for a frame of any size up to 2 KiB, so that no frame
/// of `writer` and no copy of it in the home gets a block. A sample of a variable
/// channel takes 4 bytes and its text, so this holds only while the texts of a test
/// are short.
pub(crate) fn fill(writer: &Writer) -> Vec<frame::Draft> {
    let mut held = Vec::new();
    for text in (0..=2_048).rev().step_by(16) {
        let series: Vec<_> = (writer.set().entries().iter().enumerate())
            .map(|(i, entry)| (i, entry.data_type.width().unwrap_or(4 + text)))
            .collect();
        let error = loop {
            match writer.draft(Form::Raw, &series) {
                Ok(draft) => held.push(draft),
                Err(error) => break error,
            }
        };
        assert!(
            matches!(error, frame::Error::Pool(block::Error::Exhausted { .. })),
            "{error}"
        );
    }
    held
}
