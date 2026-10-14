use clock::Clock;
use clock::log::{Mesh, Reader, Time};
use estimate::Measurement;
use sim::node::Node;
use types::time::{Span, Stamp};

use crate::common::{ms, node};

const MS: i64 = 1_000_000;

/// A reader of `node` whose mesh time has its midpoint `apart` ns from the wall time
/// that the node starts with, with the clock that serves it. The sim clocks do not
/// move.
fn reader(node: &Node, apart: i64) -> (Clock, Reader) {
    let (mut clock, mesh) = Clock::new(node.clock());
    let source = clock.add();
    let at = node.clock().now();
    let monotonic = i64::try_from(at.0).expect("fits");
    let offset = Span::from_nanos(wall(apart).nanos() - monotonic);
    let measurement = Measurement::new(at, offset, ms(2)).expect("at most 36500 days");
    clock.push(source, measurement);
    (clock, Reader::new(node.wall(), mesh))
}

/// The wall time that a node starts with, moved by `nanos`.
fn wall(nanos: i64) -> Stamp {
    Stamp::from_nanos(sim::node::Config::default().wall.nanos() + nanos)
}

#[test]
fn has_no_mesh_time_with_no_sources() {
    let (_sim, node) = node();
    let (_clock, mesh) = Clock::new(node.clock());
    let reader = Reader::new(node.wall(), mesh);
    assert_eq!(
        reader.now(),
        Time {
            wall: wall(0),
            mesh: Mesh::Unknown
        }
    );
}

#[test]
fn a_midpoint_at_most_1_ms_from_the_wall_time_is_near() {
    for apart in [0, MS, -MS] {
        let (_sim, node) = node();
        let (_clock, reader) = reader(&node, apart);
        assert_eq!(
            reader.now(),
            Time {
                wall: wall(0),
                mesh: Mesh::Near
            },
            "{apart} ns"
        );
    }
}

#[test]
fn a_midpoint_more_than_1_ms_from_the_wall_time_is_apart() {
    for apart in [MS + 1, -MS - 1, Span::HOUR.nanos()] {
        let (_sim, node) = node();
        let (_clock, reader) = reader(&node, apart);
        let mesh = Mesh::Apart(wall(apart));
        assert_eq!(
            reader.now(),
            Time {
                wall: wall(0),
                mesh
            },
            "{apart} ns"
        );
    }
}

#[test]
fn a_step_of_the_wall_clock_leaves_mesh_time_apart() {
    let (_sim, node) = node();
    let (_clock, reader) = reader(&node, 0);
    let second = Span::SECOND.nanos();
    node.step_wall(Span::from_nanos(2 * second));
    let stepped = Time {
        wall: wall(2 * second),
        mesh: Mesh::Apart(wall(0)),
    };
    assert_eq!(reader.now(), stepped);
}

#[test]
fn a_clone_reads_the_same_clocks() {
    let (_sim, node) = node();
    let (_clock, reader) = reader(&node, 5 * MS);
    let clone = reader.clone();
    node.step_wall(ms(5));
    let stepped = Time {
        wall: wall(5 * MS),
        mesh: Mesh::Near,
    };
    assert_eq!(clone.now(), stepped);
    assert_eq!(reader.now(), clone.now());
}
