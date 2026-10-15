//! The bound of steps of each `mesh` test, which each test binary and the lib tests
//! include.

/// The most steps of one call of `Sim::run` or `Sim::run_for`: 10 times the most of a
/// correct run of any `mesh` test, 158,516 steps.
const MAX: u64 = 1_600_000;

/// The config of the sim of a `mesh` test with `seed`. A call that waits past
/// [`MAX`] steps fails with `sim::Error::Steps`.
pub(crate) fn simulation(seed: u64) -> sim::Config {
    sim::Config {
        seed,
        steps_max: MAX,
        ..sim::Config::default()
    }
}
