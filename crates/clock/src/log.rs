//! The times of each log line of a node.

use types::time::Stamp;

/// The most that the best guess of mesh time can be from the wall time, in
/// nanoseconds, for [`Mesh::Near`].
const NEAR: u64 = 1_000_000;

/// Reads the times of each log line of a node: its OS wall time, and mesh time
/// beside it. For a person who reads the log, never for a data stamp. Clones read
/// the same clocks.
#[derive(Clone, Debug)]
pub struct Reader {
    wall: env::wall::Wall,
    mesh: crate::Reader,
}

impl Reader {
    /// Reads `wall`, the node's OS wall clock, and the mesh time of `mesh`.
    #[must_use]
    pub fn new(wall: env::wall::Wall, mesh: crate::Reader) -> Self {
        Self { wall, mesh }
    }

    /// Reads both clocks once.
    #[must_use]
    pub fn now(&self) -> Time {
        #[expect(clippy::disallowed_methods, reason = "clock reads the OS clock")]
        let wall = self.wall.now().time;
        let mesh = match self.mesh.best() {
            None => Mesh::Unknown,
            Some(best) if best.nanos().abs_diff(wall.nanos()) <= NEAR => Mesh::Near,
            Some(best) => Mesh::Apart(best),
        };
        Time { wall, mesh }
    }
}

/// The times of one log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Time {
    /// The OS wall time: the stamp of the line.
    pub wall: Stamp,
    /// Mesh time beside `wall`.
    pub mesh: Mesh,
}

/// Mesh time beside the wall time of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mesh {
    /// The best guess of mesh time is at most 1 ms from the wall time. The time
    /// between the reads of the two clocks counts toward the 1 ms.
    Near,
    /// The best guess of mesh time, which is more than 1 ms from the wall time.
    Apart(Stamp),
    /// The node has no mesh time yet.
    Unknown,
}
