//! The exit tests of ONE NODE: one `foundation` node reads a simulated OPC UA server
//! and pushes each sample to a simulated InfluxDB. Exit test 1 runs on the real binary
//! with `cargo test -p node --test it samples_from_an_opc_ua_server_reach_influxdb`.

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
/// so that a node that reads too few fails the test: half the ticks of `rate = "10Hz"`.
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

/// The reads that the server answered of one variable, from the first to the end of
/// the changes, and the samples that InfluxDB holds of them to the last step of the
/// value. A later read can stand in for the read of the last step: only a change
/// steps the value and sets the status, so it has the same status.
#[derive(Debug)]
struct Trace {
    served: Vec<Sample>,
    stored: Vec<Sample>,
    /// How many of `served` the server answered before the changes.
    unchanged: usize,
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

/// Waits until InfluxDB holds a sample of each channel of `variables`.
fn first_stored(plant: &Plant, variables: &[(&str, &str)]) {
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
}

/// Changes each variable of `variables` [`TICKS`] times with `quality`, and gives the
/// reads of each. Panics when the server answered fewer than [`READS`] reads of a
/// variable in the changes.
fn change(
    opcua: &Opcua,
    variables: &[(&'static str, &'static str)],
    quality: Quality,
) -> Vec<Reads> {
    let count = |served: &BTreeMap<String, Vec<Sample>>, variable| {
        served.get(variable).map_or(0, Vec::len)
    };
    let before = opcua.served();
    opcua.change(TICKS, quality);
    let after = opcua.served();
    let reads: Vec<_> = variables
        .iter()
        .map(|&(variable, channel)| Reads {
            variable,
            channel,
            unchanged: count(&before, variable),
            changed: count(&after, variable),
        })
        .collect();
    for reads in &reads {
        let in_changes = reads.changed - reads.unchanged;
        let channel = reads.channel;
        assert!(
            in_changes >= READS,
            "{channel}: {in_changes} reads in the changes"
        );
    }
    reads
}

/// Waits until InfluxDB holds each of `reads`, and gives the trace of each.
fn held(plant: &Plant, reads: &[Reads]) -> Vec<Trace> {
    plant.rig.wait("InfluxDB holds each read", || {
        let stored = plant.influx.stored();
        caught_up(reads, plant.opcua.served(), stored)
    })
}

/// The trace of each of `reads`, once `stored` (by channel) holds each read of
/// `served` (by variable) to the last step of the value in the changes. Sample `k`
/// stored has the value of read `k`, at a time within [`BOUND`] of it. A lost or extra
/// sample before a step puts a sample on a read of another value. From the last step
/// on, a later read of the same value can stand in for a lost read. `served` is taken
/// after `stored`. Else `Err` with the first sample that is not its read.
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
            let end = (1..reads.changed)
                .rev()
                .find(|&k| served[k].value.to_bits() != served[k - 1].value.to_bits())
                .unwrap_or_else(|| {
                    panic!(
                        "{channel}: no read of the changes steps the value: {:?}",
                        &served[..reads.changed]
                    )
                });
            for (k, (sample, read)) in
                stored.iter().zip(&served).take(end + 1).enumerate()
            {
                if sample.value.to_bits() != read.value.to_bits()
                    || (sample.time - read.time).nanos().abs() > BOUND.nanos()
                {
                    return Err(format!(
                        "{channel}: sample {k} stored, {sample:?}, is not read {k}, \
                         {read:?}"
                    ));
                }
            }
            if stored.len() <= end {
                return Err(format!(
                    "{channel}: InfluxDB holds {} of the {} reads",
                    stored.len(),
                    end + 1
                ));
            }
            Ok(Trace {
                served: served[..reads.changed].to_vec(),
                stored: stored[..=end].to_vec(),
                unchanged: reads.unchanged,
            })
        })
        .collect()
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

/// Three reads of `plant.spike` before the changes, three in them, and one after the
/// last change, at 10 Hz.
fn create_served() -> Vec<Sample> {
    let values = [0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 3.0];
    (10..)
        .zip(values)
        .map(|(at, value)| Sample {
            time: Stamp::from_nanos(at * 100 * Span::MILLISECOND.nanos()),
            value,
            quality: None,
        })
        .collect()
}

/// [`caught_up`] of `stored` against `served`.
fn check(served: &[Sample], stored: &[Sample]) -> Result<Vec<Trace>, String> {
    let reads = [Reads {
        variable: "ns=3;s=SpikeData",
        channel: "plant.spike",
        unchanged: 3,
        changed: 6,
    }];
    caught_up(
        &reads,
        BTreeMap::from([("ns=3;s=SpikeData".into(), served.to_vec())]),
        BTreeMap::from([("plant.spike".into(), stored.to_vec())]),
    )
}

/// `samples`, each `by` later.
fn later(samples: &[Sample], by: Span) -> Vec<Sample> {
    let later = |sample: &Sample| Sample {
        time: sample.time + by,
        ..*sample
    };
    samples.iter().map(later).collect()
}

#[test]
fn a_trace_holds_each_read_to_the_last_step_within_the_bound() {
    let served = create_served();
    let mut stepped = served.clone();
    stepped[6].value = 4.0;
    let mut tick = served.clone();
    tick[3].time = tick[3].time + Span::from_nanos(90 * Span::MILLISECOND.nanos());
    let cases = [
        (served.clone(), served.clone()),
        (served.clone(), served[..6].to_vec()),
        (stepped.clone(), stepped.clone()),
        (served.clone(), later(&served, BOUND)),
        (
            served.clone(),
            later(&served, Span::from_nanos(-BOUND.nanos())),
        ),
        (
            tick.clone(),
            later(&tick, Span::from_nanos(6 * Span::MILLISECOND.nanos())),
        ),
    ];
    for (served, stored) in cases {
        let traces = check(&served, &stored).expect("caught up");
        let pairs = (traces.iter())
            .map(|trace| (&trace.served[..], &trace.stored[..], trace.unchanged));
        assert_eq!(pairs.collect::<Vec<_>>(), [(&served[..6], &stored[..6], 3)]);
    }
}

#[test]
fn a_lost_extra_or_late_sample_fails_the_trace() {
    let served = create_served();
    let not_read = |k, stored: &Sample| {
        format!(
            "plant.spike: sample {k} stored, {stored:?}, is not read {k}, {:?}",
            served[k]
        )
    };
    let extra = Sample {
        time: served[4].time + Span::from_nanos(50 * Span::MILLISECOND.nanos()),
        ..served[4]
    };
    let stale = |k: usize| Sample {
        time: served[k].time,
        ..served[4]
    };
    let (mut late, mut early) = (served.clone(), served.clone());
    late[0].time = late[0].time + BOUND + Span::from_nanos(1);
    early[0].time = early[0].time - BOUND - Span::from_nanos(1);
    let cases = [
        (
            served[1..].to_vec(),
            not_read(2, &served[3]),
            "the first read lost",
        ),
        (
            [&served[..1], &served[2..]].concat(),
            not_read(2, &served[3]),
            "a read before the changes lost",
        ),
        (
            [&served[..4], &served[5..]].concat(),
            not_read(4, &served[5]),
            "the last read before the last change lost",
        ),
        (
            [&served[..2], &served[1..]].concat(),
            not_read(3, &served[2]),
            "an extra sample",
        ),
        (
            [&served[..5], &[extra], &served[5..]].concat(),
            not_read(5, &extra),
            "an extra sample before the last step",
        ),
        (
            [&served[..5], &[stale(5), stale(6)]].concat(),
            not_read(5, &stale(5)),
            "the value before the last step in place of the last value",
        ),
        (late.clone(), not_read(0, &late[0]), "a late time"),
        (early.clone(), not_read(0, &early[0]), "an early time"),
        (
            served[..5].to_vec(),
            "plant.spike: InfluxDB holds 5 of the 6 reads".into(),
            "not caught up",
        ),
        (
            vec![],
            "plant.spike: InfluxDB holds 0 of the 6 reads".into(),
            "none stored",
        ),
    ];
    for (stored, error, case) in cases {
        assert_eq!(check(&served, &stored).unwrap_err(), error, "{case}");
    }
}

#[test]
fn a_lost_read_fails_when_the_last_value_came_before() {
    let mut served = create_served();
    served[5].value = 1.0;
    served[6].value = 1.0;
    let stored = [&served[..4], &served[5..]].concat();
    assert_eq!(
        check(&served, &stored).unwrap_err(),
        format!(
            "plant.spike: sample 4 stored, {:?}, is not read 4, {:?}",
            served[5], served[4]
        )
    );
}

#[test]
fn a_trace_with_no_step_panics_with_the_reads_of_the_changes() {
    let served = vec![create_served()[0]; 7];
    let panic = std::panic::catch_unwind(|| check(&served, &served))
        .expect_err("a trace with no step panics");
    let reads = &served[..6];
    assert_eq!(
        panic.downcast_ref::<String>().map(String::as_str),
        Some(&*format!(
            "plant.spike: no read of the changes steps the value: {reads:?}"
        ))
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
fn samples_from_an_opc_ua_server_reach_influxdb() {
    let plant = create_plant(PLANT);
    first_stored(&plant, &VARIABLES);
    let reads = change(&plant.opcua, &VARIABLES, Quality::GOOD);
    for trace in held(&plant, &reads) {
        let good = |sample: &Sample| sample.quality == Some(Quality::GOOD);
        assert!(trace.served.iter().all(good), "{trace:?}");
    }
}

#[test]
#[ignore = "waits on #435, #2510"]
fn a_bad_status_keeps_its_quality() {
    let plant = create_plant(QUALITY);
    let spike = &VARIABLES[..1];
    first_stored(&plant, spike);
    let traces = held(&plant, &change(&plant.opcua, spike, BAD));
    let [trace] = &traces[..] else {
        panic!("one variable: {traces:?}");
    };
    let qualities = |samples: &[Sample]| -> Vec<Option<Quality>> {
        samples.iter().map(|sample| sample.quality).collect()
    };
    let served = qualities(&trace.served);
    // One read can fall between the count of the reads and the first change.
    let (before, changes) = served.split_at(trace.unchanged);
    assert!(
        before.iter().all(|quality| *quality == Some(Quality::GOOD))
            && changes[1..].iter().all(|quality| *quality == Some(BAD)),
        "Good before the changes, then Bad: {trace:?}"
    );
    assert_eq!(qualities(&trace.stored), served[..trace.stored.len()]);
}

#[test]
#[ignore = "waits on #435"]
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
#[ignore = "waits on #435"]
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
    let failing = rig.wait("`influx` shows the failed write", || {
        let status = rig.status();
        status
            .get("influx")
            .filter(|connector| connector.error.is_some())
            .cloned()
            .ok_or_else(|| format!("{status:#?}"))
    });
    assert_eq!(
        failing,
        Connector {
            name: "influx".into(),
            kind: "influx".into(),
            state: "running".into(),
            address: Some(format!("http://{influx_address}")),
            restarts: Some(0),
            counts: Some(BTreeMap::from([("confirmed".into(), 0)])),
            error: Some(Error {
                class: "retry".into(),
                text: format!(
                    "the write to InfluxDB failed: the connect failed: {influx_address} \
                     refused the connection"
                ),
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
    let mut plant = Plant { rig, opcua, influx };
    let reads = change(&plant.opcua, &VARIABLES, Quality::GOOD);
    plant.influx.serve();
    held(&plant, &reads);
}

#[test]
#[ignore = "waits on #435"]
fn status_shows_samples_flow() {
    let plant = create_plant(PLANT);
    let rig = &plant.rig;
    let before = rig.wait("both counts are above 0", || counts_above(rig, (0, 0)));
    plant.opcua.change(TICKS, Quality::GOOD);
    rig.wait("both counts grow", || counts_above(rig, before));
}
