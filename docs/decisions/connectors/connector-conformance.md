- **CONNECTOR CONFORMANCE (#2247, #2641)** `oracles/conformance/connector/` holds the
  scenarios that each connector kind passes, the oracle of D4's conformance kit. Each
  kind crate runs them as its `conformance` test, from `<kind>/main.rs`: a `Device`
  and one `suite!` line, which expands to one `#[test]` for each scenario. The
  `[[test]]` entry of each kind is part of the oracle. The scenarios use only the
  public rig of `connector::testing` and the public sim, and each runs two sim nodes:
  the plant (the rig and the connector) and the device (the kind's simulator). A
  `Device` gives plain inputs: its table, a setup that defines what the connector
  needs and starts each writer that feeds it, its simulator, the name of its progress
  count, its backoff form, and the configs that its check refuses with their exact
  diagnostics. People own the scenarios. A kind that fails one has a defect, or the
  person changes the scenario.

  "Heals" means: within `HEAL` (the backoff cap of 1 min, plus 10 s), the state is
  Running, the error is gone, and the progress count grows past its value at the start
  of the heal. The backoff form is `Restart` (the run ends, and the supervisor shows
  Restarting with restarts that grow) or `Run` (the run goes on: Running, class Retry,
  0 restarts, and no progress, as INFLUX KIND locks for `influx`). Each scenario
  checks the form that the kind names, and refuses the other one.

  1. **Starts.** Within 10 s: Running, no error, 0 restarts, and progress above 0.
  2. **Device absent at start.** No simulator. Within 1 min, the backoff form, never
     class Config. `Restart`: Restarting, class Device or Retry, and restarts that
     grow to 2. `Run`: Running, class Retry, 0 restarts, and progress 0, held for
     10 s. Then the simulator starts: heals.
  3. **Device lost while running**, in two forms: the simulator stops (each
     connection closed), and the device node crashes (`Sim::crash`, no close reaches
     the plant). Within 30 s: an error of class Device or Retry. Then the device
     comes back: heals, and no count and no restart count went down.
  4. **Stop.** `Run::stop` gives `Ok` within 5 s while Running, in the backoff form,
     and while the plant connects over a link that delays each packet by 1 h (the
     plant holds 1 stream before the stop). After it, the plant node holds no open
     stream. The connect form with a link that loses each packet (loss 1) comes back
     with #3040.
  5. **Config.** For each refused config, `Table::check` gives exactly its
     diagnostics, and `Rig::supervise` ends with the same `Error::Config`. For a kind
     whose check gives writes, a write channel that the hub does not define gives
     Stopped, class Config, 0 restarts, and the text of `connector.unknown-channel`.

  Scenarios 6 (random faults) and 7 (no secret in a status) come in #2642. The
  numbers are tunable by the architect of `connector`. Plan of `laptop.architect-2`
  (https://github.com/synnaxlabs/foundation/issues/2247#issuecomment-6155477394), with
  the fixes of 2026-10-17T16:05:31Z
  (https://github.com/synnaxlabs/foundation/issues/2641#issuecomment-6236306639, item
  5 for the delay of 1 h). The backoff form: proposed by `box1.builder-2`
  (https://github.com/synnaxlabs/foundation/issues/2641#issuecomment-6241104680). Lost:
  the scenarios in a module of `connector`, where an agent could change an oracle in a
  normal PR; and scenarios that each kind writes with shared assertion helpers, where
  nothing makes a kind run each scenario.
