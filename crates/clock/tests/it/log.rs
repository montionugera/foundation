use clock::Clock;
use clock::log::{Mesh, Reader, Time};
use estimate::Measurement;
use sim::node::Node;
use types::time::{Span, Stamp};

use crate::common::{ms, node, run};

const MS_NS: i64 = 1_000_000;

/// A reader of `node` whose mesh time has its best guess `apart` ns from the wall
/// time that the node starts with, with the clock that serves it. The sim clocks do
/// not move.
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

/// The times of a line from `reader`.
fn read(reader: &Reader) -> Time {
    #[expect(clippy::disallowed_methods, reason = "the tests read the log times")]
    reader.now()
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
        read(&reader),
        Time {
            wall: wall(0),
            mesh: Mesh::Unsynced
        }
    );
}

#[test]
fn a_best_guess_at_most_2_ms_from_the_wall_time_is_near() {
    for apart in [0, 2 * MS_NS, -2 * MS_NS] {
        let (_sim, node) = node();
        let (_clock, reader) = reader(&node, apart);
        assert_eq!(
            read(&reader),
            Time {
                wall: wall(0),
                mesh: Mesh::Near
            },
            "{apart} ns"
        );
    }
}

#[test]
fn a_best_guess_more_than_2_ms_from_the_wall_time_is_apart() {
    for apart in [2 * MS_NS + 1, -2 * MS_NS - 1, Span::HOUR.nanos()] {
        let (_sim, node) = node();
        let (_clock, reader) = reader(&node, apart);
        let mesh = Mesh::Apart(wall(apart));
        assert_eq!(
            read(&reader),
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
    assert_eq!(read(&reader), stepped);
}

#[test]
fn a_clone_reads_the_same_clocks() {
    let (_sim, node) = node();
    let (_clock, reader) = reader(&node, 5 * MS_NS);
    let clone = reader.clone();
    node.step_wall(ms(5));
    let stepped = Time {
        wall: wall(5 * MS_NS),
        mesh: Mesh::Near,
    };
    assert_eq!(read(&clone), stepped);
    assert_eq!(read(&reader), read(&clone));
}

/// The times of a line on a lone node whose OS wall clock starts at `wall`, with
/// mesh time from its one OS wall source.
fn lone(wall: Stamp, wall_error: Option<Span>) -> Time {
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config {
        wall,
        wall_error,
        ..sim::node::Config::default()
    });
    let (mut clock, mesh) = Clock::new(node.clock());
    let source = clock.add();
    #[expect(clippy::disallowed_methods, reason = "the test feeds the mesh clock")]
    let measurement = clock::source::Wall::new(node.wall(), node.clock()).measure();
    clock.push(source, measurement);
    read(&Reader::new(node.wall(), mesh))
}

#[test]
fn a_lone_node_with_no_os_bound_after_the_last_edge_that_fits_is_near() {
    let wall: Stamp = "2200-01-01T00:00:00Z".parse().expect("a stamp");
    assert_eq!(
        lone(wall, None),
        Time {
            wall,
            mesh: Mesh::Near
        }
    );
}

#[test]
fn a_lone_node_with_no_os_bound_before_the_first_edge_that_fits_is_near() {
    let wall: Stamp = "1700-01-01T00:00:00Z".parse().expect("a stamp");
    assert_eq!(
        lone(wall, None),
        Time {
            wall,
            mesh: Mesh::Near
        }
    );
}

#[test]
fn a_lone_node_with_a_bound_past_the_end_of_a_stamp_is_near() {
    let wall = Stamp::from_nanos(i64::MAX - 5 * MS_NS);
    assert_eq!(
        lone(wall, Some(Span::from_nanos(10 * MS_NS))),
        Time {
            wall,
            mesh: Mesh::Near
        }
    );
}

/// A lone node whose OS wall clock runs 200 ppm fast, the most that the clock allows
/// for, with an OS bound that grows 500 µs each second between updates, as between
/// the updates of chrony.
#[test]
fn a_lone_node_at_200_ppm_with_a_growing_os_bound_is_near() {
    let (mut sim, host) = node();
    let mesh = run(&host, host.wall());
    let reader = Reader::new(host.wall(), mesh);
    for second in 0..400 {
        let error = 10 * MS_NS + 500_000 * (second % 64);
        host.set_wall_error(Some(Span::from_nanos(error)));
        host.step_wall(Span::from_nanos(200_000));
        sim.run_for(Span::SECOND).expect("runs");
        assert_eq!(read(&reader).mesh, Mesh::Near, "second {second}");
    }
}

/// The times of a line on a node whose OS wall clock starts at `wall`, with one
/// source that puts mesh time at `mesh` ns. The sim clocks do not move.
fn near_the_last_stamp(wall: Stamp, mesh: i128) -> Time {
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config {
        wall,
        ..sim::node::Config::default()
    });
    let (mut clock, reader) = Clock::new(node.clock());
    let source = clock.add();
    let at = node.clock().now();
    let offset = i64::try_from(mesh - i128::from(at.0)).expect("fits");
    let measurement = Measurement::new(at, Span::from_nanos(offset), ms(2))
        .expect("at most 36500 days");
    clock.push(source, measurement);
    read(&Reader::new(node.wall(), reader))
}

#[test]
fn mesh_time_past_the_last_stamp_is_apart() {
    let wall = Stamp::from_nanos(i64::MAX - MS_NS);
    let past = i128::from(i64::MAX) + i128::from(Span::HOUR.nanos());
    assert_eq!(
        near_the_last_stamp(wall, past),
        Time {
            wall,
            mesh: Mesh::Apart(Stamp::from_nanos(i64::MAX))
        }
    );
}

#[test]
fn a_wall_time_at_the_last_stamp_is_apart() {
    let wall = Stamp::from_nanos(i64::MAX);
    let best = i64::MAX - MS_NS;
    assert_eq!(
        near_the_last_stamp(wall, i128::from(best)),
        Time {
            wall,
            mesh: Mesh::Apart(Stamp::from_nanos(best))
        }
    );
}
