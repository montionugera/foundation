"""Times `Writer.write` and `Writer.write_series` against a node process.

Run it with the `wheel` build of the package on the path, NumPy installed, and
`FOUNDATION_NODE` set to the path of a release `foundation` binary:

    FOUNDATION_NODE=target/release/foundation python3 sdk/python/bench/write.py

It prints, in microseconds:

- the round trip of `write` (one value, node stamps) and of `write_series` (a series
  of `f64` and its stamps), p50 and p99;
- the copy of `write_series` alone: a call that copies the stamps and the `f64`
  series and then raises at a third array of another length, less that call with
  arrays of one sample, p50;
- `write` with channel names as keys against places as keys, with no send, p50;
- the part of a call before `sdk::write::Writer::write`: the key lookup, the
  conversion, and the copy. A call that does each of these and then raises before
  the send, less the p50 of a call that only raises, p99.
"""

import os
import statistics
import subprocess
import sys
import time

import numpy as np

TESTS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tests")
sys.path.insert(0, TESTS)
from test_client import PATIENCE, Node  # noqa: E402

import foundation  # noqa: E402

CHANNELS = 256
PLACES = (1, 16, 64, 255)
SAMPLES = (100, 1000, 3000)
SENDS = 400
CALLS = 3000


def p50(times: list[int]) -> float:
    """The median of `times`, in nanoseconds, in microseconds."""
    return statistics.median(times) / 1e3


def p99(times: list[int]) -> float:
    """The 99th percentile of `times`, in nanoseconds, in microseconds."""
    return statistics.quantiles(times, n=100)[98] / 1e3


def timed(call, count: int, raises=None) -> list[int]:
    """The time of each of `count` calls of `call`, in nanoseconds, after 50 calls to
    warm up. A call may raise `raises`."""
    times = []
    for n in range(count + 50):
        began = time.perf_counter_ns()
        try:
            call()
        except raises or ():
            pass
        if n >= 50:
            times.append(time.perf_counter_ns() - began)
    return times


def channel(n: int) -> str:
    """The name of channel `n`."""
    return f"w.c{n:03}"


def connect(node: Node) -> foundation.Client:
    """Plans and applies `CHANNELS` channels of `f64` on one index on `node`, and
    gives a client of it once the hub of the node knows them."""
    hcl = 'channel "w.time" { kind = "index" }\n'
    for n in range(CHANNELS):
        hcl += f'channel "{channel(n)}" {{\n  data_type = "f64"\n'
        hcl += '  index = "w.time"\n}\n'
    hcl += 'placement "w.home" {\n  select = "w.*"\n  home = "edge"\n}\n'
    with open(os.path.join(node.dir.name, "w.hcl"), "w") as file:
        file.write(hcl)
    for args in (["plan", "w.hcl", "--out", "w.plan"], ["apply", "w.plan"]):
        subprocess.run(
            [os.environ["FOUNDATION_NODE"], *args],
            cwd=node.dir.name,
            check=True,
            capture_output=True,
            timeout=PATIENCE,
        )
    client = node.connect()
    deadline = time.monotonic() + PATIENCE
    while True:
        try:
            client.open_writer(channel(CHANNELS - 1)).close()
            return client
        except foundation.Stopped as error:
            if error.code != "unknown" or time.monotonic() > deadline:
                raise
            time.sleep(0.01)


def round_trips(client: foundation.Client) -> None:
    """Prints the round trip of `write` and of `write_series`."""
    random = np.random.default_rng(3320)
    with client.open_writer(channel(0), on_loss="ignore") as writer:
        times = timed(lambda: writer.write(1.5), SENDS)
        print(f"write, 1 value: round trip p50 {p50(times):.1f}, p99 {p99(times):.1f}")
        stamp = [time.time_ns() + 1_000_000_000]
        for samples in SAMPLES:
            floats = random.random(samples)

            def send() -> None:
                stamps = np.arange(stamp[0], stamp[0] + samples, dtype=np.int64)
                stamp[0] += samples
                writer.write_series({"w.time": stamps, channel(0): floats})

            times = timed(send, SENDS)
            print(
                f"write_series, {samples} samples: round trip p50 {p50(times):.1f}, "
                f"p99 {p99(times):.1f}"
            )


def copies(client: foundation.Client) -> None:
    """Prints the copy of `write_series` alone."""
    random = np.random.default_rng(3320)
    with client.open_writer([channel(0), channel(1)]) as writer:
        for samples in SAMPLES:
            stamps = np.arange(samples, dtype=np.int64)
            floats = random.random(samples)

            def failing(length: int) -> float:
                short = np.zeros(length + 1)
                series = {
                    "w.time": stamps[:length],
                    channel(0): floats[:length],
                    channel(1): short,
                }
                call = lambda: writer.write_series(series)  # noqa: E731
                return p50(timed(call, CALLS, ValueError))

            copy = failing(samples) - failing(1)
            print(f"write_series, {samples} samples: copy p50 {copy:.2f}")


def keys(client: foundation.Client) -> None:
    """Prints `write` with names as keys against places as keys, with no send."""
    for places in PLACES:
        names = [channel(n) for n in range(places)]
        with client.open_writer(names) as writer:
            by_name = {name: 1.5 for name in names} | {"w.time": "x"}
            by_place = {n: 1.5 for n in range(places)} | {places: "x"}
            name = p50(timed(lambda: writer.write(by_name), CALLS, TypeError))
            place = p50(timed(lambda: writer.write(by_place), CALLS, TypeError))
            print(
                f"write, {places} places, no send: names p50 {name:.2f}, places p50 "
                f"{place:.2f}, names less places {name - place:.2f}"
            )


def parts(client: foundation.Client) -> None:
    """Prints the part of `write` and of `write_series` before the call of `sdk`."""
    random = np.random.default_rng(3320)
    for places in PLACES:
        names = [channel(n) for n in range(places)]
        with client.open_writer(names) as writer:
            base = p50(timed(lambda: writer.write({"w.time": "x"}), CALLS, TypeError))
            values = {name: 1.5 for name in names} | {"w.time": "x"}
            times = timed(lambda: writer.write(values), CALLS, TypeError)
            part = p99([t - base * 1e3 for t in times])
            print(f"write, {places} places: before the send p99 {part:.2f}")
            # Place 0 again, with a series of another length, raises after the copy.
            raised = {0: np.zeros(2), names[0]: np.zeros(3)}
            base = p50(timed(lambda: writer.write_series(raised), CALLS, ValueError))
            for samples in SAMPLES:
                series = {name: random.random(samples) for name in names}
                series["w.time"] = np.arange(samples, dtype=np.int64)
                series[0] = np.zeros(samples + 1)
                times = timed(lambda: writer.write_series(series), CALLS, ValueError)
                part = p99([t - base * 1e3 for t in times])
                print(
                    f"write_series, {places} places, {samples} samples: before the "
                    f"send p99 {part:.2f}"
                )


def main() -> None:
    node = Node()
    try:
        client = connect(node)
        round_trips(client)
        copies(client)
        keys(client)
        parts(client)
        client.close()
    finally:
        node.stop()


if __name__ == "__main__":
    main()
