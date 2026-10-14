//! The times of each log line of a node.

use types::time::Stamp;

/// The most that the best guess of mesh time can be from the wall time, in
/// nanoseconds, for [`Mesh::Near`].
const NEAR: u64 = 2_000_000;

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
            None => Mesh::Unsynced,
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
    /// The best guess is at most 2 ms from the wall time. The two clocks are read one
    /// after the other, so the time between the reads counts toward the 2 ms.
    Near,
    /// Mesh time's best guess, which is more than 2 ms from the wall time.
    Apart(Stamp),
    /// No majority of the sources has agreed yet.
    Unsynced,
}
