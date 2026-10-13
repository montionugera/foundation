//! The exit tests of ONE NODE: one `foundation` node reads a simulated OPC UA server
//! and pushes each sample to a simulated InfluxDB. Exit test 1 runs on the real binary
//! with `cargo test -p node --test it samples_from_an_opc_ua_server_reach_influxdb`.

mod sim;

use std::collections::BTreeMap;
use std::net::SocketAddr;

use types::quality::Quality;
use types::time::Span;

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

/// What the server served of one variable, and as many of the first samples of its
/// channel as InfluxDB holds.
#[derive(Debug)]
struct Trace {
    channel: &'static str,
    served: Vec<Sample>,
    stored: Vec<Sample>,
}

impl Trace {
    /// Asserts that InfluxDB holds the value of each served sample, in order, each at a
    /// time within [`BOUND`] of the time it was served.
    fn assert_held(&self) {
        let values = |samples: &[Sample]| -> Vec<u64> {
            samples
                .iter()
                .map(|sample| sample.value.to_bits())
                .collect()
        };
        let channel = self.channel;
        assert_eq!(
            values(&self.stored),
            values(&self.served),
            "{channel}: the values in order"
        );
        for (served, stored) in self.served.iter().zip(&self.stored) {
            assert!(
                (stored.time - served.time).nanos().abs() <= BOUND.nanos(),
                "{channel}: {stored:?} is more than {BOUND} from {served:?}"
            );
        }
    }
}

/// Waits until InfluxDB holds a sample of each channel of `variables`.
fn wait_first(plant: &Plant, variables: &[(&str, &'static str)]) {
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

/// Changes each variable [`TICKS`] times with `quality`, and gives each read that the
/// server answered of each variable of `variables`, by its channel. Panics when the
/// server answered fewer than [`READS`] reads of a variable.
fn change(
    plant: &Plant,
    variables: &[(&str, &'static str)],
    quality: Quality,
) -> BTreeMap<&'static str, Vec<Sample>> {
    plant.opcua.change(TICKS, quality);
    let mut served = plant.opcua.served();
    variables
        .iter()
        .map(|&(variable, channel)| {
            let reads = served
                .remove(variable)
                .expect("invariant: the server serves each variable");
            assert!(reads.len() >= READS, "{channel}: {} reads", reads.len());
            (channel, reads)
        })
        .collect()
}

/// The trace of each channel of `served`, once InfluxDB holds as many samples of each
/// as the server served. Else `Err` with the counts.
fn caught_up(
    plant: &Plant,
    served: &BTreeMap<&'static str, Vec<Sample>>,
) -> Result<Vec<Trace>, String> {
    let mut stored = plant.influx.stored();
    let traces: Vec<_> = served
        .iter()
        .map(|(&channel, served)| Trace {
            channel,
            served: served.clone(),
            stored: stored.remove(channel).unwrap_or_default(),
        })
        .collect();
    if traces
        .iter()
        .all(|trace| trace.stored.len() >= trace.served.len())
    {
        return Ok(traces
            .into_iter()
            .map(|mut trace| {
                trace.stored.truncate(trace.served.len());
                trace
            })
            .collect());
    }
    let counts: Vec<_> = traces
        .iter()
        .map(|trace| (trace.channel, trace.served.len(), trace.stored.len()))
        .collect();
    Err(format!("each channel, served, and stored: {counts:?}"))
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
    wait_first(&plant, &VARIABLES);
    let served = change(&plant, &VARIABLES, Quality::GOOD);
    for trace in plant
        .rig
        .wait("InfluxDB holds each read", || caught_up(&plant, &served))
    {
        trace.assert_held();
    }
}

#[test]
#[ignore = "waits on #337, #435, #1734, #1744, #2326, #2401"]
fn a_bad_status_keeps_its_quality() {
    let plant = create_plant(QUALITY);
    let spike = &VARIABLES[..1];
    wait_first(&plant, spike);
    let served = change(&plant, spike, BAD);
    let traces = plant
        .rig
        .wait("InfluxDB holds each read", || caught_up(&plant, &served));
    let [trace] = &traces[..] else {
        panic!("one variable: {traces:?}");
    };
    let qualities = |samples: &[Sample]| -> Vec<Option<Quality>> {
        samples.iter().map(|sample| sample.quality).collect()
    };
    let served = qualities(&trace.served);
    assert_eq!(served.last(), Some(&Some(BAD)), "{trace:?}");
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
    let mut plant = Plant { rig, opcua, influx };
    let served = change(&plant, &VARIABLES, Quality::GOOD);
    plant.influx.serve();
    for trace in plant
        .rig
        .wait("InfluxDB holds each read", || caught_up(&plant, &served))
    {
        trace.assert_held();
    }
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
