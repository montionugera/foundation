//! The exit tests of ONE NODE: one `foundation` node reads a simulated OPC UA server
//! and pushes each sample to a simulated InfluxDB. Exit test 1 runs on the real binary
//! with `cargo test -p node --test it samples_from_an_opc_ua_server_reach_influxdb --
//! --include-ignored`.

mod sim;

use std::collections::BTreeMap;
use std::net::SocketAddr;

use types::quality::Quality;
use types::time::{Span, Stamp};

use self::sim::{Influx, Opcua, Sample};
use crate::rig::Rig;
use crate::status::{Connector, Error};
use crate::text;

/// The config of the quickstart page.
const PLANT: &str = include_str!("../../../acceptance/tests/it/fixtures/plant.hcl");

/// The channel `plant.spike`, with the quality channel `plant.quality`.
const QUALITY: &str = include_str!("one_node/quality.hcl");

/// Each variable of the OPC UA server, and the channel that reads it in each config.
const VARIABLES: [(&str, &str); 3] = [
    ("ns=3;s=SpikeData", "plant.spike"),
    ("ns=3;s=DipData", "plant.dip"),
    ("ns=3;s=PositiveTrendData", "plant.trend"),
];

/// The changes of each variable: 5 s at 10 Hz.
const TICKS: usize = 50;

/// The fewest reads of each variable that the server answers in the [`TICKS`] changes,
/// so that a node that reads too few fails the test: half the ticks of `rate = 10`.
const READS: usize = TICKS / 2;

/// The MVP target of the time error bound.
const BOUND: Span = Span::SECOND;

/// `Bad`, with no subcode.
const BAD: Quality = Quality(0x8000_0000);

/// A node, the OPC UA server that it reads, and the InfluxDB that it pushes to.
#[derive(Debug)]
struct Plant {
    rig: Rig,
    opcua: Opcua,
    influx: Influx,
}

/// A running node on `hcl`, with both servers serving.
fn create_plant(hcl: &str) -> Plant {
    let mut opcua = Opcua::default();
    let mut influx = Influx::default();
    let mut rig = Rig::new();
    rig.config(&served(hcl, opcua.serve(), influx.serve()));
    rig.start();
    rig.apply();
    Plant { rig, opcua, influx }
}

/// `hcl` with `opcua` in place of `localhost:50000` and `influx` in place of
/// `localhost:8086`.
fn served(hcl: &str, opcua: SocketAddr, influx: SocketAddr) -> String {
    hcl.replace("localhost:50000", &opcua.to_string())
        .replace("localhost:8086", &influx.to_string())
}

/// The reads that the server answered of one variable, from the read of the first
/// sample that InfluxDB holds of its channel to the last change, and the samples stored
/// of them.
#[derive(Debug)]
struct Trace {
    served: Vec<Sample>,
    stored: Vec<Sample>,
}

/// How many reads of one variable the server answered before and to the end of the
/// changes.
#[derive(Debug)]
struct Reads {
    variable: &'static str,
    channel: &'static str,
    unchanged: usize,
    changed: usize,
}

/// Waits until InfluxDB holds a sample of each channel of `variables`, changes each
/// variable [`TICKS`] times with `quality`, and waits until InfluxDB holds each read
/// that the server answered from then to the last change. Gives the trace of each
/// variable. A trace starts at the first sample stored, since a connector's reader can
/// open after the first reads. Panics when the server answered fewer than [`READS`]
/// reads of a variable in the changes.
fn held(
    plant: &Plant,
    variables: &[(&'static str, &'static str)],
    quality: Quality,
) -> Vec<Trace> {
    plant.rig.wait("InfluxDB holds the first values", || {
        let stored = plant.influx.stored();
        let empty: Vec<_> = variables
            .iter()
            .map(|&(_, channel)| channel)
            .filter(|channel| stored.get(*channel).is_none_or(Vec::is_empty))
            .collect();
        if empty.is_empty() {
            Ok(())
        } else {
            Err(format!("no sample of {empty:?}"))
        }
    });
    let count = |served: &BTreeMap<String, Vec<Sample>>, variable| {
        served.get(variable).map_or(0, Vec::len)
    };
    let unchanged = plant.opcua.served();
    plant.opcua.change(TICKS, quality);
    let changed = plant.opcua.served();
    let reads: Vec<_> = variables
        .iter()
        .map(|&(variable, channel)| {
            let reads = Reads {
                variable,
                channel,
                unchanged: count(&unchanged, variable),
                changed: count(&changed, variable),
            };
            let changes = reads.changed - reads.unchanged;
            assert!(
                changes >= READS,
                "{channel}: {changes} reads in the changes"
            );
            reads
        })
        .collect();
    plant.rig.wait("InfluxDB holds each read", || {
        let stored = plant.influx.stored();
        caught_up(&reads, plant.opcua.served(), stored)
    })
}

/// The trace of each of `reads`, once `stored` (by channel) holds each read of
/// `served` (by variable) to the end of the changes, from one before the changes. Each
/// sample stored is the read nearest to it in time, with its value, within [`BOUND`],
/// so a lost read fails even when the next read has its value. `served` is taken after
/// `stored`, so it holds the read of each sample stored. Else `Err` with the first
/// sample that is not its read.
fn caught_up(
    reads: &[Reads],
    mut served: BTreeMap<String, Vec<Sample>>,
    mut stored: BTreeMap<String, Vec<Sample>>,
) -> Result<Vec<Trace>, String> {
    reads
        .iter()
        .map(|reads| {
            let channel = reads.channel;
            let served = served.remove(reads.variable).unwrap_or_default();
            let stored = stored.remove(channel).unwrap_or_default();
            let nearest = |sample: &Sample| nearest(&served, sample.time);
            let first = (stored.first().map(nearest))
                .filter(|&first| first < reads.unchanged)
                .ok_or_else(|| {
                    format!(
                        "{channel}: the first sample stored is {:?}, not one of the {} \
                         reads before the changes",
                        stored.first(),
                        reads.unchanged
                    )
                })?;
            let held = reads.changed - first;
            for (k, sample) in stored.iter().take(held).enumerate() {
                let read = &served[first + k];
                if nearest(sample) != first + k
                    || sample.value.to_bits() != read.value.to_bits()
                    || (sample.time - read.time).nanos().abs() > BOUND.nanos()
                {
                    return Err(format!(
                        "{channel}: sample {k} stored, {sample:?}, is not read {}, \
                         {read:?}",
                        first + k
                    ));
                }
            }
            if stored.len() < held {
                return Err(format!(
                    "{channel}: InfluxDB holds {} of the {held} reads from read {first}",
                    stored.len()
                ));
            }
            Ok(Trace {
                served: served[first..reads.changed].to_vec(),
                stored: stored[..held].to_vec(),
            })
        })
        .collect()
}

/// The index of the read of `served`, in order of time, nearest to `time`, the
/// earlier of two at the same distance.
fn nearest(served: &[Sample], time: Stamp) -> usize {
    let after = served.partition_point(|read| read.time < time);
    let distance = |i: usize| (served[i].time - time).nanos().abs();
    match after {
        0 => 0,
        after if after == served.len() || distance(after - 1) <= distance(after) => {
            after - 1
        }
        after => after,
    }
}

/// The count `key` of the connector `name` in `status`.
fn count(status: &BTreeMap<String, Connector>, name: &str, key: &str) -> Option<u64> {
    status.get(name)?.counts.as_ref()?.get(key).copied()
}

/// `in` of `plc` and `confirmed` of `influx`, once both are above `floor`.
fn counts_above(rig: &Rig, floor: (u64, u64)) -> Result<(u64, u64), String> {
    let status = rig.status();
    match (
        count(&status, "plc", "in"),
        count(&status, "influx", "confirmed"),
    ) {
        (Some(read), Some(confirmed)) if read > floor.0 && confirmed > floor.1 => {
            Ok((read, confirmed))
        }
        _ => Err(format!("{status:#?}")),
    }
}

#[test]
fn a_trace_pairs_each_stored_sample_with_the_nearest_read() {
    let sample = |at: i64, value| Sample {
        time: Stamp::from_nanos(at * 3 * Span::SECOND.nanos()),
        value,
        quality: None,
    };
    // Three reads before the changes, three in them, and one after the last change.
    let served: Vec<_> = [0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 3.0]
        .into_iter()
        .zip(0..)
        .map(|(value, at)| sample(at, value))
        .collect();
    let reads = [Reads {
        variable: "ns=3;s=SpikeData",
        channel: "plant.spike",
        unchanged: 3,
        changed: 6,
    }];
    let check = |stored: &[Sample]| {
        caught_up(
            &reads,
            BTreeMap::from([("ns=3;s=SpikeData".into(), served.clone())]),
            BTreeMap::from([("plant.spike".into(), stored.to_vec())]),
        )
    };
    let traces = check(&served[1..]).expect("caught up");
    let [trace] = &traces[..] else {
        panic!("one variable: {traces:?}");
    };
    assert_eq!(
        (&trace.served[..], &trace.stored[..]),
        (&served[1..6], &served[1..6])
    );
    let not_read = |k, stored: &Sample, read| {
        format!(
            "plant.spike: sample {k} stored, {stored:?}, is not read {read}, {:?}",
            served[read]
        )
    };
    assert_eq!(
        check(&served[3..]).unwrap_err(),
        format!(
            "plant.spike: the first sample stored is {:?}, not one of the 3 reads \
             before the changes",
            Some(&served[3])
        ),
        "the first read after the changes started"
    );
    assert_eq!(
        check(&[]).unwrap_err(),
        "plant.spike: the first sample stored is None, not one of the 3 reads before \
         the changes",
        "none stored"
    );
    assert_eq!(
        check(&[&served[..1], &served[2..]].concat()).unwrap_err(),
        not_read(1, &served[2], 1),
        "a lost read before the changes"
    );
    assert_eq!(
        check(&[&served[..5], &served[6..]].concat()).unwrap_err(),
        not_read(5, &served[6], 5),
        "a lost last read"
    );
    let mut late = served.clone();
    late[0].time = late[0].time + BOUND + Span::from_nanos(1);
    assert_eq!(
        check(&late).unwrap_err(),
        not_read(0, &late[0], 0),
        "a time out of bound"
    );
    assert_eq!(
        check(&served[..4]).unwrap_err(),
        "plant.spike: InfluxDB holds 4 of the 6 reads from read 0",
        "not caught up"
    );
}

#[test]
fn each_config_names_the_served_addresses() {
    let opcua = SocketAddr::from(([127, 0, 0, 1], 4840));
    let influx = SocketAddr::from(([127, 0, 0, 1], 9086));
    for hcl in [PLANT, QUALITY] {
        let hcl = served(hcl, opcua, influx);
        assert!(hcl.contains("\"opc.tcp://127.0.0.1:4840\""), "{hcl}");
        assert!(hcl.contains("\"http://127.0.0.1:9086\""), "{hcl}");
        assert!(!hcl.contains("localhost"), "{hcl}");
    }
}

#[test]
#[ignore = "waits on #337, #435, #1734, #1744, #2326, #2401"]
fn samples_from_an_opc_ua_server_reach_influxdb() {
    let plant = create_plant(PLANT);
    held(&plant, &VARIABLES, Quality::GOOD);
}

#[test]
#[ignore = "waits on #337, #435, #1734, #1744, #2326, #2401, #2510"]
fn a_bad_status_keeps_its_quality() {
    let plant = create_plant(QUALITY);
    let spike = &VARIABLES[..1];
    let traces = held(&plant, spike, BAD);
    let [trace] = &traces[..] else {
        panic!("one variable: {traces:?}");
    };
    let qualities = |samples: &[Sample]| -> Vec<Option<Quality>> {
        samples.iter().map(|sample| sample.quality).collect()
    };
    let served = qualities(&trace.served);
    let bad: Vec<_> = (served.iter())
        .skip_while(|quality| **quality == Some(Quality::GOOD))
        .collect();
    assert!(
        bad.len() >= READS && bad.iter().all(|quality| **quality == Some(BAD)),
        "Good, then only Bad: {trace:?}"
    );
    assert_eq!(qualities(&trace.stored), served);
}

#[test]
#[ignore = "waits on #337, #435, #1734, #1744, #2401"]
fn a_wrong_config_gives_its_file_line_and_fix() {
    let mut rig = Rig::new();
    let hcl = PLANT
        .replacen("data_type", "datatype", 1)
        .replace("\"http://localhost:8086\"", "8086");
    rig.config(&hcl);
    rig.start();
    let output = rig.run(&["plan", "plant.hcl", "--out", "plant.plan"], b"");
    assert_eq!(
        (
            output.status.code(),
            text(&output.stdout),
            text(&output.stderr)
        ),
        (
            Some(2),
            "",
            "error[document.missing-attribute]: the `channel` block has no `data_type`\n  \
             --> plant.hcl:5:1\n\
             fix: Add a `data_type` attribute such as \"f64\"\n\
             \n\
             error[document.unknown-attribute]: `datatype` is not an attribute of the \
             `channel` block\n  \
             --> plant.hcl:6:3\n\
             fix: Use `kind`, `data_type`, `index`, `quality`, or `unit`, or remove it\n\
             \n\
             error[connector.bad-uri]: a URI is a string, not an integer\n  \
             --> plant.hcl:37:17\n\
             fix: Write an `http` URI such as \"http://10.0.0.2:8086\"\n"
        )
    );
    assert!(!rig.dir.join("plant.plan").exists(), "plan writes no plan");
}

#[test]
#[ignore = "waits on #337, #435, #1734, #1735, #1744, #2326, #2401"]
fn a_wrong_endpoint_shows_in_status_and_heals() {
    let mut opcua = Opcua::default();
    let mut influx = Influx::default();
    let mut rig = Rig::new();
    let opcua_address = opcua.serve();
    let influx_address = influx.serve();
    influx.stop();
    rig.config(&served(PLANT, opcua_address, influx_address));
    rig.start();
    rig.apply();
    let restarting = rig.wait("`influx` restarts", || {
        let status = rig.status();
        status
            .get("influx")
            .filter(|connector| {
                connector.state == "restarting" && connector.restarts >= Some(1)
            })
            .cloned()
            .ok_or_else(|| format!("{status:#?}"))
    });
    let next = restarting
        .error
        .as_ref()
        .and_then(|error| error.next.clone());
    assert!(next.is_some(), "{restarting:#?}");
    assert_eq!(
        restarting,
        Connector {
            kind: "influx".into(),
            state: "restarting".into(),
            address: Some(format!("http://{influx_address}")),
            restarts: restarting.restarts,
            counts: Some(BTreeMap::from([("confirmed".into(), 0)])),
            error: Some(Error {
                class: "device".into(),
                text: format!(
                    "the connect failed: {influx_address} refused the connection"
                ),
                next,
            }),
        }
    );
    rig.wait("`plc` reads the first value of each variable", || {
        let status = rig.status();
        match count(&status, "plc", "in") {
            Some(read) if read >= 3 => Ok(read),
            _ => Err(format!("{status:#?}")),
        }
    });
    influx.serve();
    held(&Plant { rig, opcua, influx }, &VARIABLES, Quality::GOOD);
}

#[test]
#[ignore = "waits on #337, #435, #1734, #1735, #1744, #2326, #2401"]
fn status_shows_samples_flow() {
    let plant = create_plant(PLANT);
    let rig = &plant.rig;
    let before = rig.wait("both counts are above 0", || counts_above(rig, (0, 0)));
    plant.opcua.change(TICKS, Quality::GOOD);
    rig.wait("both counts grow", || counts_above(rig, before));
}
