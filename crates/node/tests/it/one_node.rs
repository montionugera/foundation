//! The exit tests of ONE NODE: one `foundation` node reads a simulated OPC UA server
//! and pushes each sample to a simulated InfluxDB. Exit test 1 runs on the real binary
//! with `cargo test -p node --test it samples_from_an_opc_ua_server_reach_influxdb`.

mod sim;

use std::collections::BTreeMap;
use std::fs::Permissions;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;

use types::quality::Quality;
use types::time::{Span, Stamp};

use self::sim::{Influx, Opcua, Sample};
use crate::quickstart::plant;
use crate::rig::Rig;
use crate::status::{Connector, Error};
use crate::text;

/// The channel `plant.spike`, with the quality channel `plant.quality`.
const QUALITY: &str = include_str!("one_node/quality.hcl");

/// Each variable of the OPC UA server, and the channel that reads it in each config.
const VARIABLES: [(&str, &str); 3] = [
    ("ns=3;s=SpikeData", "plant.spike"),
    ("ns=3;s=DipData", "plant.dip"),
    ("ns=3;s=PositiveTrendData", "plant.trend"),
];

/// The changes of each variable: 2 s at 10 Hz. Each change steps the value.
const TICKS: usize = 20;

/// The changes of each variable in the test of the time bound: 5 s at 10 Hz, so that a
/// time error that grows slowly passes [`BOUND`].
const TRACED: usize = 50;

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
    /// The time of `opcua` before the node started.
    started: Stamp,
}

/// A running node on `hcl`, with both servers serving.
fn create_plant(hcl: &str) -> Plant {
    let mut opcua = Opcua::default();
    let mut influx = Influx::default();
    let mut rig = Rig::new();
    rig.config(&served(hcl, opcua.serve(), influx.serve()));
    let started = opcua.now();
    rig.start();
    rig.apply();
    Plant {
        rig,
        opcua,
        influx,
        started,
    }
}

/// The quickstart config, with the token of `influx` from `source`.
fn with_token(source: &str) -> String {
    let select = "  select      = \"plant.*\"\n";
    let hcl =
        plant().replacen(select, &format!("{select}  token       = {source}\n"), 1);
    assert_ne!(hcl, plant(), "the quickstart config selects `plant.*`");
    hcl
}

/// A running node on [`with_token`] of `source`, with `INFLUX_TOKEN` set to `t0ken`,
/// which pushes to an InfluxDB that takes only `t0ken`. No config applies yet.
fn create_token_plant(source: impl FnOnce(&Rig) -> String) -> Plant {
    let mut opcua = Opcua::default();
    let mut influx = Influx::taking("t0ken");
    let mut rig = Rig::new();
    rig.var("INFLUX_TOKEN", "t0ken");
    let hcl = with_token(&source(&rig));
    rig.config(&served(&hcl, opcua.serve(), influx.serve()));
    let started = opcua.now();
    rig.start();
    Plant {
        rig,
        opcua,
        influx,
        started,
    }
}

/// Asserts that `plan` of `rig` exits with one `secret.unresolved` problem at the
/// token of [`with_token`], with `message`, and writes no plan.
fn refuses_the_token(rig: &Rig, message: &str) {
    let hcl = std::fs::read_to_string(rig.dir.join("plant.hcl")).expect("reads it");
    let line = 1
        + (hcl.lines())
            .position(|line| line.starts_with("  token "))
            .expect("the config has a token");
    let output = rig.run(&["plan", "plant.hcl", "--out", "plant.plan"], b"");
    let expected = format!(
        "error[secret.unresolved]: {message}\n  --> plant.hcl:{line}:17\nfix: Give the \
         node the value at the source, or change the source, then plan again\n"
    );
    assert_eq!(
        (
            output.status.code(),
            text(&output.stdout),
            text(&output.stderr)
        ),
        (Some(2), "", &*expected)
    );
    assert!(!rig.dir.join("plant.plan").exists(), "plan writes no plan");
}

/// The error that `influx` shows, once it shows one.
fn influx_error(rig: &Rig) -> Error {
    rig.wait("`influx` shows an error", || {
        let status = rig.status();
        (status.get("influx").and_then(|influx| influx.error.clone()))
            .ok_or_else(|| format!("{status:#?}"))
    })
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

/// Changes each variable of `variables` `ticks` times with `quality`, and gives the
/// reads of each. Panics when the server answered fewer than half of `ticks` reads of a
/// variable in the changes, so that a node that reads too few at `rate = "10Hz"` fails.
fn change(
    opcua: &Opcua,
    variables: &[(&'static str, &'static str)],
    quality: Quality,
    ticks: usize,
) -> Vec<Reads> {
    let count = |served: &BTreeMap<String, Vec<Sample>>, variable| {
        served.get(variable).map_or(0, Vec::len)
    };
    let before = opcua.served();
    opcua.change(ticks, quality);
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
            in_changes >= ticks / 2,
            "{channel}: {in_changes} reads in the changes"
        );
    }
    reads
}

/// Waits until InfluxDB holds each of `reads`, and gives the trace of each.
fn held(plant: &Plant, reads: &[Reads]) -> Vec<Trace> {
    plant.rig.wait("InfluxDB holds each read", || {
        let stored = plant.influx.stored();
        caught_up(reads, plant.started, plant.opcua.served(), stored)
    })
}

/// The trace of each of `reads`, once `stored` (by channel) holds each read of
/// `served` (by variable) to the last step of the value in the changes. Sample `k`
/// stored has the value of read `k`, at a time [`within`] its limit. A lost or extra
/// sample before a step puts a sample on a read of another value. From the last step
/// on, a later read of the same value can stand in for a lost read. The node started
/// at `started`, and `served` is taken after `stored`. Else `Err` with the first sample
/// that is not its read, and the offset and window of each sample.
fn caught_up(
    reads: &[Reads],
    started: Stamp,
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
            if served.len() <= end + 1 {
                return Err(format!(
                    "{channel}: the server answered no read after read {end}"
                ));
            }
            let before =
                std::iter::once(started).chain(served.iter().map(|read| read.time));
            let windows = before
                .zip(&served[1..])
                .map(|(before, after)| after.time - before);
            let checked: Vec<_> = (stored.iter().zip(&served).zip(windows))
                .take(end + 1)
                .map(|((sample, read), window)| {
                    (sample, read, sample.time - read.time, window)
                })
                .collect();
            for (k, &(sample, read, offset, window)) in checked.iter().enumerate() {
                if sample.value.to_bits() != read.value.to_bits()
                    || !within(k, offset, window)
                {
                    let offsets: Vec<_> = (checked.iter().enumerate())
                        .map(|(k, (.., offset, window))| {
                            format!("{k}: {offset} in {window}")
                        })
                        .collect();
                    return Err(format!(
                        "{channel}: sample {k} stored, {sample:?}, is not read {k}, \
                         {read:?}\nthe offset of each sample in its window: {}",
                        offsets.join(", ")
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

/// Whether a sample `offset` from read `k` is within its limit. The window of a read is
/// the time from the read before it, or from the start of the node, to the read after
/// it. A stamp is the midpoint of its read, so the hold, half the window plus
/// [`BOUND`], holds each sample. A read after the first whose window is at most
/// `2 * BOUND` takes the MVP target, [`BOUND`].
fn within(k: usize, offset: Span, window: Span) -> bool {
    let offset = offset.nanos().abs();
    if k >= 1 && window.nanos() <= 2 * BOUND.nanos() {
        offset <= BOUND.nanos()
    } else {
        2 * offset <= window.nanos() + 2 * BOUND.nanos()
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

/// [`caught_up`] of `stored` against `served`, from a node that started at 0.9 s, one
/// read before the first of [`create_served`].
fn check(served: &[Sample], stored: &[Sample]) -> Result<Vec<Trace>, String> {
    let reads = [Reads {
        variable: "ns=3;s=SpikeData",
        channel: "plant.spike",
        unchanged: 3,
        changed: 6,
    }];
    caught_up(
        &reads,
        Stamp::from_nanos(900 * Span::MILLISECOND.nanos()),
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

/// `samples`, with sample `k` `by` later.
fn moved(samples: &[Sample], k: usize, by: Span) -> Vec<Sample> {
    let mut moved = samples.to_vec();
    moved[k].time = moved[k].time + by;
    moved
}

/// [`create_served`], with each read after read `k` later, so that the window of read
/// `k` is `window`.
fn widened(k: usize, window: Span) -> Vec<Sample> {
    let served = create_served();
    let by = window.nanos() - (served[k + 1].time - served[k - 1].time).nanos();
    let (before, after) = served.split_at(k + 1);
    [before, &later(after, Span::from_nanos(by))].concat()
}

/// `by` and `-by`.
fn both(by: i64) -> [Span; 2] {
    [Span::from_nanos(by), Span::from_nanos(-by)]
}

/// The first line of `error`.
fn first(error: &str) -> &str {
    error.lines().next().unwrap_or_default()
}

/// The hold of read 0 in [`check`]: half its window of 200 ms, plus [`BOUND`].
const HOLD: i64 = 100 * Span::MILLISECOND.nanos() + BOUND.nanos();

/// A window over `2 * BOUND`, and its hold.
const WIDE: (Span, i64) = (
    Span::from_nanos(2 * BOUND.nanos() + 2),
    2 * BOUND.nanos() + 1,
);

#[test]
fn a_trace_holds_each_read_to_the_last_step_within_the_bound() {
    let served = create_served();
    let mut stepped = served.clone();
    stepped[6].value = 4.0;
    let mut tick = served.clone();
    tick[3].time = tick[3].time + Span::from_nanos(90 * Span::MILLISECOND.nanos());
    let wide = widened(2, WIDE.0);
    let mut cases = vec![
        (served.clone(), served.clone()),
        (served.clone(), served[..6].to_vec()),
        (stepped.clone(), stepped.clone()),
        (
            tick.clone(),
            later(&tick, Span::from_nanos(6 * Span::MILLISECOND.nanos())),
        ),
    ];
    for by in both(BOUND.nanos()) {
        cases.push((served.clone(), later(&served, by)));
    }
    for by in both(HOLD) {
        cases.push((served.clone(), moved(&served, 0, by)));
    }
    for by in both(WIDE.1) {
        cases.push((wide.clone(), moved(&wide, 2, by)));
    }
    for (served, stored) in cases {
        let traces = check(&served, &stored).expect("caught up");
        let pairs = (traces.iter())
            .map(|trace| (&trace.served[..], &trace.stored[..], trace.unchanged));
        assert_eq!(pairs.collect::<Vec<_>>(), [(&served[..6], &stored[..6], 3)]);
    }
}

#[test]
fn a_lost_or_extra_sample_fails_the_trace() {
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
        let failed = check(&served, &stored).unwrap_err();
        assert_eq!(first(&failed), error, "{case}");
    }
}

#[test]
fn a_time_over_its_limit_fails_the_trace() {
    let served = create_served();
    let edge = widened(2, Span::from_nanos(2 * BOUND.nanos()));
    let wide = widened(2, WIDE.0);
    let cases = [
        (&served, 0, HOLD + 1),
        (&served, 1, BOUND.nanos() + 1),
        (&edge, 2, BOUND.nanos() + 1),
        (&wide, 2, WIDE.1 + 1),
    ];
    for (served, k, over) in cases {
        for by in both(over) {
            let stored = moved(served, k, by);
            let failed = check(served, &stored).unwrap_err();
            let error = format!(
                "plant.spike: sample {k} stored, {:?}, is not read {k}, {:?}",
                stored[k], served[k]
            );
            assert_eq!(first(&failed), error, "{by} at {k}");
        }
    }
}

#[test]
fn a_failed_trace_gives_the_offset_and_window_of_each_sample() {
    let served = widened(2, Span::from_nanos(500 * Span::MILLISECOND.nanos()));
    let offset = Span::from_nanos(5 * Span::MILLISECOND.nanos());
    let stored = moved(&later(&served, offset), 1, BOUND);
    assert_eq!(
        check(&served, &stored).unwrap_err(),
        format!(
            "plant.spike: sample 1 stored, {:?}, is not read 1, {:?}\nthe offset of \
             each sample in its window: 0: 5ms in 200ms, 1: 1.005s in 200ms, 2: 5ms in \
             500ms, 3: 5ms in 500ms, 4: 5ms in 200ms, 5: 5ms in 200ms",
            stored[1], served[1]
        )
    );
}

#[test]
fn a_trace_waits_for_the_read_after_the_last_step() {
    let served = &create_served()[..6];
    assert_eq!(
        check(served, served).unwrap_err(),
        "plant.spike: the server answered no read after read 5"
    );
}

#[test]
fn a_lost_read_fails_when_the_last_value_came_before() {
    let mut served = create_served();
    served[5].value = 1.0;
    served[6].value = 1.0;
    let stored = [&served[..4], &served[5..]].concat();
    assert_eq!(
        first(&check(&served, &stored).unwrap_err()),
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
    for hcl in [plant(), QUALITY] {
        let hcl = served(hcl, opcua, influx);
        assert!(hcl.contains("\"opc.tcp://127.0.0.1:4840\""), "{hcl}");
        assert!(hcl.contains("\"http://127.0.0.1:9086\""), "{hcl}");
        assert!(!hcl.contains("localhost"), "{hcl}");
    }
}

#[test]
fn samples_from_an_opc_ua_server_reach_influxdb() {
    let plant = create_plant(plant());
    first_stored(&plant, &VARIABLES);
    let reads = change(&plant.opcua, &VARIABLES, Quality::GOOD, TRACED);
    for trace in held(&plant, &reads) {
        let good = |sample: &Sample| sample.quality == Some(Quality::GOOD);
        assert!(trace.served.iter().all(good), "{trace:?}");
    }
}

#[test]
fn a_bad_value_is_left_out_and_counted() {
    let plant = create_plant(QUALITY);
    let spike = &VARIABLES[..1];
    first_stored(&plant, spike);
    let [reads] = &change(&plant.opcua, spike, BAD, TICKS)[..] else {
        panic!("one variable");
    };
    // One read can fall between the count of the reads and the first change.
    let bad = u64::try_from(reads.changed - reads.unchanged - 1).expect("a few");
    plant.rig.wait("`bad` of `plc` counts the Bad reads", || {
        match count(&plant.rig.status(), "plc", "bad") {
            Some(counted) if counted >= bad => Ok(()),
            counted => Err(format!("{counted:?} of at least {bad}")),
        }
    });
    let mut served = plant.opcua.served();
    let served = served.remove(reads.variable).unwrap_or_default();
    let before = &served[..=reads.unchanged];
    let good = |sample: &Sample| sample.quality == Some(Quality::GOOD);
    // InfluxDB holds no status, so a sample is Good when its read is.
    let mut stored = plant.influx.stored();
    for sample in stored.remove(reads.channel).unwrap_or_default() {
        let read = |read: &Sample| {
            good(read)
                && read.value.to_bits() == sample.value.to_bits()
                && (sample.time - read.time).nanos().abs() <= BOUND.nanos()
        };
        assert!(
            before.iter().any(read),
            "{sample:?} is a Good read before the changes: {before:?}"
        );
    }
}

#[test]
fn a_wrong_config_gives_its_file_line_and_fix() {
    let mut rig = Rig::new();
    let hcl = plant()
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
             fix: Write an `http` or `https` URI such as \"http://10.0.0.2:8086\"\n"
        )
    );
    assert!(!rig.dir.join("plant.plan").exists(), "plan writes no plan");
}

#[test]
fn a_wrong_endpoint_shows_in_status_and_heals() {
    let mut opcua = Opcua::default();
    let mut influx = Influx::default();
    let mut rig = Rig::new();
    let opcua_address = opcua.serve();
    let influx_address = influx.serve();
    influx.stop();
    rig.config(&served(plant(), opcua_address, influx_address));
    let started = opcua.now();
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
            counts: Some(BTreeMap::from([
                ("confirmed".into(), 0),
                ("lost".into(), 0),
                ("oversized".into(), 0),
            ])),
            error: Some(Error {
                class: "retry".into(),
                text: format!(
                    "the write to InfluxDB failed: the connect failed: {influx_address} \
                     refused the connection"
                ),
            }),
            note: None,
        }
    );
    rig.wait("`plc` reads the first value of each variable", || {
        let status = rig.status();
        match count(&status, "plc", "in") {
            Some(read) if read >= 3 => Ok(read),
            _ => Err(format!("{status:#?}")),
        }
    });
    let mut plant = Plant {
        rig,
        opcua,
        influx,
        started,
    };
    let reads = change(&plant.opcua, &VARIABLES, Quality::GOOD, TICKS);
    plant.influx.serve();
    held(&plant, &reads);
}

#[test]
fn a_token_from_the_environment_of_the_node_reaches_influxdb() {
    let plant = create_token_plant(|_| "env(\"INFLUX_TOKEN\")".into());
    plant.rig.apply();
    first_stored(&plant, &VARIABLES);
}

#[test]
fn a_plan_refuses_a_token_from_a_variable_that_the_node_does_not_set() {
    let plant = create_token_plant(|_| "env(\"PLANT_TOKEN\")".into());
    refuses_the_token(
        &plant.rig,
        "the secret env(\"PLANT_TOKEN\"): the environment variable PLANT_TOKEN of the \
         node is not set. Start the node with it set to the value",
    );
}

#[test]
fn a_token_file_shows_its_fix_until_it_holds_the_token() {
    for named in [false, true] {
        let mut given = None;
        let plant = create_token_plant(|rig| {
            let (source, path) = if named {
                let path = rig.dir.join("foundation-data/data/secrets/plant.token");
                ("secret(\"plant.token\")".to_owned(), path)
            } else {
                let path = rig.dir.join("token");
                (format!("file({:?})", path.display().to_string()), path)
            };
            given = Some((source.clone(), path));
            source
        });
        let (source, path) = given.expect("the plant names its source");
        let shown = path.display();
        let hidden = if named {
            format!(", unreadable by other users: chmod o-r {shown}")
        } else {
            String::new()
        };
        let missing = format!(
            "the secret {source}: the file {shown} is not there. Write the value to \
             it{hidden}"
        );
        refuses_the_token(&plant.rig, &missing);
        let dir = path.parent().expect("a directory");
        std::fs::create_dir_all(dir).expect("makes the directory");
        std::fs::write(&path, b"t0ken\n").expect("writes the token");
        std::fs::set_permissions(&path, Permissions::from_mode(0o600))
            .expect("makes the token private");
        let output = plant
            .rig
            .run(&["plan", "plant.hcl", "--out", "plant.plan"], b"");
        assert!(output.status.success(), "{}", text(&output.stderr));
        // A run reads the token again, so it shows a file that goes after the plan.
        std::fs::remove_file(&path).expect("removes the token");
        let output = plant.rig.run(&["apply", "plant.plan"], b"");
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert_eq!(
            influx_error(&plant.rig),
            Error {
                class: "retry".into(),
                text: missing,
            }
        );
        std::fs::write(&path, b"t0ken\n").expect("writes the token");
        std::fs::set_permissions(&path, Permissions::from_mode(0o600))
            .expect("makes the token private");
        first_stored(&plant, &VARIABLES);
    }
}

#[test]
fn status_shows_samples_flow() {
    let plant = create_plant(plant());
    let rig = &plant.rig;
    // The node reads each variable at each tick, also when its value does not change.
    let before = rig.wait("both counts are above 0", || counts_above(rig, (0, 0)));
    rig.wait("both counts grow", || counts_above(rig, before));
}

/// `browse` of the OPC UA connector gives the points of a folder of the server, with
/// their facts and values.
#[test]
fn browse_gives_the_points_of_the_server_with_their_values() {
    let plant = create_plant(plant());
    let args = [
        "browse",
        "plc",
        "plant.hcl",
        "--start",
        "Plant/Line",
        "--values",
        "--json",
    ];
    let output = plant.rig.run(&args, b"");
    assert_eq!((output.status.code(), text(&output.stderr)), (Some(0), ""));
    let found: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json");
    let entries = found["entries"].as_array().expect("a list");
    let temperature = entries
        .iter()
        .find(|entry| entry["path"] == "Plant/Line/Temperature")
        .unwrap_or_else(|| panic!("no Temperature in {found}"));
    let expected = serde_json::json!({
        "path": "Plant/Line/Temperature",
        "tag": "ns=1;s=Plant.Line.Temperature",
        "shape": "point",
        "type": "f64",
        "unit": "°C",
        "unit_code": "CEL",
        "access": "read",
        "value": 21.5,
    });
    assert_eq!(*temperature, expected);
}
