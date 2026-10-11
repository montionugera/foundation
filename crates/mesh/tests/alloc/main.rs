//! A spec that `mesh` reads costs one build of its tree, in the root check of
//! `spec::region::definitions`. This binary has no test harness: the count covers each
//! thread, and a harness allocates on its own thread at any time.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

#[path = "../common/mod.rs"]
mod common;

use std::collections::BTreeMap;

use common::{PRIVATE_KEY, create_config};
use mesh::Mesh;
use sim::Sim;
use spec::definition::{Definition, Kind};
use spec::subject::Subject;
use spec::tree::Chunks;
use types::name::Name;
use types::time::Span;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

/// The definitions of the smaller spec. The larger has twice as many.
const FEW: usize = 512;
/// The tenths of a tree build that the allocations of an apply and a read of one more
/// definition stay under. One build at the apply and one in the root check of the
/// read give 42.8, and a second build at the read gives 52.8.
const BUILDS: u64 = 48;

fn main() {
    let tree = |count| {
        let definitions = create_definitions(count);
        ALLOCATOR
            .count(|| spec::region::tree(&mut Chunks::default(), &definitions))
            .1
    };
    let mesh = applied(2 * FEW) - applied(FEW);
    let tree = tree(2 * FEW) - tree(FEW);
    assert!(
        mesh * 10 < tree * BUILDS,
        "an apply and a read of {FEW} more definitions make {mesh} more allocations, \
         and a build of their tree {tree}",
    );
}

/// The allocations of a run that applies `count` definitions and reads the spec.
fn applied(count: usize) -> u64 {
    let ((), allocations) = ALLOCATOR.count(|| {
        let mut sim = Sim::new(sim::Config::default());
        let node = sim.node(sim::node::Config::default());
        let ran = sim.run_on(&node, move |node, tasks| async move {
            let mesh = Mesh::open(create_config(&node, &tasks).await)
                .await
                .expect("the mesh opens");
            node.clock()
                .sleep(Span::from_nanos(Span::SECOND.nanos()))
                .await;
            let definitions = create_definitions(count);
            let applied = mesh.apply(mesh.pointer(), definitions, BTreeMap::new());
            applied.await.expect("the change takes effect");
            drop(mesh.spec().await.expect("the mesh runs"));
        });
        assert_eq!(ran, Ok(()), "the run ends");
    });
    allocations
}

/// `count` subjects, each with the public key of the node.
fn create_definitions(count: usize) -> BTreeMap<Name, Definition> {
    let subject = Subject::new(vec![PRIVATE_KEY.public()]).expect("one key");
    (0..count)
        .map(|at| {
            let key = Kind::Subject.key(&format!("plant.s{at}")).expect("a name");
            (key, Definition::Subject(subject.clone()))
        })
        .collect()
}
