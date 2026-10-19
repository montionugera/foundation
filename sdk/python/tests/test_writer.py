"""The writer of the Python SDK against a node process.

`FOUNDATION_NODE` is the path of the `foundation` binary that each test starts.
"""

import os
import subprocess
import threading
import time
import unittest
import uuid

import numpy as np

import foundation
from test_client import PATIENCE, Node, fork

CHANNELS = """
channel "plant.time" { kind = "index" }
channel "plant.a" {
  data_type = "f64"
  index = "plant.time"
}
channel "plant.b" {
  data_type = "i64"
  index = "plant.time"
}
channel "plant.c" {
  data_type = "u8"
  index = "plant.time"
}
channel "plant.d" {
  data_type = "bool"
  index = "plant.time"
}
channel "plant.e" {
  data_type = "uuid"
  index = "plant.time"
}
channel "plant.f" {
  data_type = "f32"
  index = "plant.time"
}
channel "plant.g" {
  data_type = "duration"
  index = "plant.time"
}
channel "other.time" { kind = "index" }
channel "other.a" {
  data_type = "f64"
  index = "other.time"
}
placement "plant.home" {
  select = "plant.*"
  home = "edge"
}
placement "other.home" {
  select = "other.*"
  home = "edge"
}
"""
UNKNOWN = (
    "the selector matches no channel, or a channel of the read or write is unknown or "
    "was removed"
)
ORDER = "the node refused the frame: a stamp is not after the stamp before it"


def planted(node: Node) -> foundation.Client:
    """Plans and applies `CHANNELS` on `node`, and gives a client of it once the hub
    of the node knows the channels, which it learns after the apply."""
    with open(os.path.join(node.dir.name, "plant.hcl"), "w") as file:
        file.write(CHANNELS)
    for args in (["plan", "plant.hcl", "--out", "plant.plan"], ["apply", "plant.plan"]):
        done = subprocess.run(
            [os.environ["FOUNDATION_NODE"], *args],
            cwd=node.dir.name,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=PATIENCE,
        )
        if done.returncode != 0 or done.stderr:
            raise AssertionError(
                f"foundation {' '.join(args)} exited with {done.returncode}:\n"
                + done.stderr.decode()
            )
    client = node.connect()
    deadline = time.monotonic() + PATIENCE
    while True:
        try:
            client.open_writer("plant.a").close()
            client.open_writer("other.a").close()
            return client
        except foundation.Stopped as error:
            if error.code != "unknown" or time.monotonic() > deadline:
                raise
            time.sleep(0.01)


class WriterTest(unittest.TestCase):
    def setUp(self) -> None:
        self.node = Node()
        self.addCleanup(self.node.stop)
        self.client = planted(self.node)
        self.addCleanup(self.client.close)

    def test_a_writer_gives_its_places_in_the_order_of_its_channels(self) -> None:
        with self.client.open_writer(["plant.b", "plant.a"]) as writer:
            self.assertEqual(
                writer.places,
                (
                    ("plant.b", "i64"),
                    ("plant.a", "f64"),
                    ("plant.time", "timestamp"),
                ),
            )
            self.assertEqual(writer.index, 2)
        with self.client.open_writer(["plant.time", "plant.a"]) as writer:
            self.assertEqual(writer.index, 0)

    def test_write_takes_frames_of_node_time_and_of_the_callers_stamps(self) -> None:
        with self.client.open_writer(["plant.a", "plant.b"]) as writer:
            self.assertIsNone(writer.write({"plant.a": 1.5, "plant.b": 2}))
            stamp = time.time_ns() + 10**9
            self.assertIsNone(writer.write({"plant.a": 2.5, 1: 3, 2: stamp}))
            with self.assertRaises(foundation.Rejected) as raised:
                writer.write({"plant.a": 3.5, "plant.time": stamp})
            self.assertEqual(raised.exception.code, "order")
            self.assertEqual(str(raised.exception), ORDER)
            self.assertIsNone(writer.write({"plant.a": 3.5, "plant.time": stamp + 1}))

    def test_write_takes_a_value_with_no_key_for_one_place(self) -> None:
        with self.client.open_writer("plant.a") as writer:
            self.assertIsNone(writer.write(2.5))
            self.assertIsNone(writer.write(3))
        with self.client.open_writer(["plant.a", "plant.b"]) as writer:
            with self.assertRaises(TypeError) as raised:
                writer.write(2.5)
            self.assertEqual(
                str(raised.exception),
                "the writer has 2 places other than the index: pass a dict from each "
                "channel name to its value",
            )

    def test_write_takes_a_value_of_each_scalar(self) -> None:
        channels = ["plant.b", "plant.c", "plant.d", "plant.e", "plant.f", "plant.g"]
        with self.client.open_writer(channels) as writer:
            values = {
                "plant.b": -(2**63),
                "plant.c": 255,
                "plant.d": True,
                "plant.e": uuid.uuid4(),
                "plant.f": 1.5,
                "plant.g": 2**63 - 1,
            }
            self.assertIsNone(writer.write(values))

    def test_write_raises_for_each_value_that_its_place_does_not_take(self) -> None:
        channels = ["plant.a", "plant.b", "plant.c", "plant.d", "plant.e", "plant.f"]
        writer = self.client.open_writer(channels)
        self.addCleanup(writer.close)
        big = 2**200
        cases = [
            ({"plant.a": "1.5"}, TypeError, "plant.a holds f64, not str"),
            ({"plant.a": True}, TypeError, "plant.a holds f64, not bool"),
            ({"plant.b": 1.5}, TypeError, "plant.b holds i64, not float"),
            ({"plant.b": False}, TypeError, "plant.b holds i64, not bool"),
            ({"plant.d": 1}, TypeError, "plant.d holds bool, not int"),
            ({"plant.e": str(uuid.uuid4())}, TypeError, "plant.e holds uuid, not str"),
            ({"plant.time": 1.0}, TypeError, "plant.time holds timestamp, not float"),
            (
                {"plant.c": 300},
                OverflowError,
                "plant.c holds u8, and 300 is outside 0 to 255",
            ),
            ({"plant.c": -1}, OverflowError, "plant.c holds u8, and -1 is outside 0 to 255"),
            (
                {"plant.b": big},
                OverflowError,
                f"plant.b holds i64, and {big} is outside -9223372036854775808 to "
                "9223372036854775807",
            ),
            (
                {"plant.f": 1e39},
                OverflowError,
                "plant.f holds f32, and 1e+39 is outside its range",
            ),
            (
                {"plant.a": 2**1024},
                OverflowError,
                f"plant.a holds f64, and {2**1024} is outside its range",
            ),
            (
                {"plant.z": 1.0},
                KeyError,
                "the writer has no place plant.z: its places are plant.a, plant.b, "
                "plant.c, plant.d, plant.e, plant.f, plant.time",
            ),
            ({9: 1.0}, ValueError, "the writer has no place 9: it has 7 places"),
            ({-1: 1.0}, ValueError, "the writer has no place -1: it has 7 places"),
            ({1.5: 1.0}, TypeError, "a key is a channel name or a place, not float"),
            (
                {"plant.a": 1.0, 0: 2.0},
                ValueError,
                "the series of the draft are not valid: entry 0 follows entry 0: "
                "entries must increase",
            ),
            ({}, ValueError, "the frame holds no sample, so it was not sent"),
        ]
        for values, kind, text in cases:
            with self.subTest(values=values):
                with self.assertRaises(kind) as raised:
                    writer.write(values)
                self.assertIs(type(raised.exception), kind)
                self.assertEqual(raised.exception.args[0], text)
        self.assertIsNone(writer.write({"plant.a": np.float64(1.5), "plant.c": 0}))
        self.assertIsNone(writer.write({"plant.f": float("nan")}))

    def test_write_series_takes_each_dtype_of_its_places(self) -> None:
        count = 1000
        start = time.time_ns()
        stamps = np.arange(start, start + count, dtype=np.int64)
        with self.client.open_writer(["plant.a", "plant.c", "plant.d"]) as writer:
            series = {
                "plant.a": np.linspace(0.0, 1.0, count),
                "plant.c": np.arange(count, dtype=np.uint8),
                "plant.d": np.arange(count) % 2 == 0,
                "plant.time": stamps,
            }
            self.assertIsNone(writer.write_series(series))
            later = (stamps + count).astype("datetime64[ns]")
            series["plant.time"] = later
            self.assertIsNone(writer.write_series(series))
            strided = {key: array[::2] for key, array in series.items()}
            strided["plant.time"] = (stamps + 2 * count)[::2]
            self.assertIsNone(writer.write_series(strided))
            with self.assertRaises(foundation.Rejected) as raised:
                writer.write_series(strided)
            self.assertEqual(raised.exception.code, "order")
            self.assertEqual(str(raised.exception), ORDER)
        with self.client.open_writer("plant.g") as writer:
            spans = np.arange(count, dtype=np.int64).astype("timedelta64[ns]")
            stamps = stamps + 3 * count
            self.assertIsNone(writer.write_series({0: spans, 1: stamps}))

    def test_write_series_raises_for_each_array_that_its_place_does_not_take(
        self,
    ) -> None:
        writer = self.client.open_writer(["plant.a", "plant.e"])
        self.addCleanup(writer.close)
        start = time.time_ns()
        stamps = np.arange(start, start + 100, dtype=np.int64)
        floats = np.zeros(100)
        cases = [
            (
                {"plant.a": floats.astype(np.float32), "plant.time": stamps},
                TypeError,
                'plant.a holds f64 and takes an array of float64, not float32: pass '
                'a.astype("float64")',
            ),
            (
                {"plant.a": floats.astype(">f8"), "plant.time": stamps},
                TypeError,
                'plant.a holds f64 and takes an array of float64, not >f8: pass '
                'a.astype("float64")',
            ),
            (
                {"plant.a": floats, "plant.time": stamps.astype("datetime64[s]")},
                TypeError,
                "plant.time holds timestamp and takes an array of int64 or "
                'datetime64[ns], not datetime64[s]: pass a.astype("datetime64[ns]")',
            ),
            (
                {"plant.a": floats, "plant.time": stamps.astype(np.int32)},
                TypeError,
                "plant.time holds timestamp and takes an array of int64 or "
                'datetime64[ns], not int32: pass a.astype("int64")',
            ),
            (
                {"plant.a": floats.reshape(10, 10), "plant.time": stamps},
                TypeError,
                "plant.a takes an array of one dimension, not 2",
            ),
            (
                {"plant.a": [0.0] * 100, "plant.time": stamps},
                TypeError,
                "plant.a takes a numpy.ndarray, not list",
            ),
            (
                {"plant.e": np.zeros(100, dtype=np.uint64), "plant.time": stamps},
                TypeError,
                "plant.e holds uuid, which the writer takes no series of",
            ),
            (
                {"plant.a": floats, "plant.time": stamps[:99]},
                ValueError,
                "the series of plant.time holds 99 samples, not the 100 of plant.a",
            ),
            (
                {"plant.a": floats},
                ValueError,
                "the series of the draft are not valid: entry 0 is present but its "
                "index, entry 2, is absent",
            ),
            ({}, ValueError, "the frame holds no sample, so it was not sent"),
        ]
        for series, kind, text in cases:
            with self.subTest(series=list(series)):
                with self.assertRaises(kind) as raised:
                    writer.write_series(series)
                self.assertIs(type(raised.exception), kind)
                self.assertEqual(str(raised.exception), text)
        large = {"plant.a": np.zeros(4000), "plant.time": np.arange(4000) + start}
        with self.assertRaises(ValueError) as raised:
            writer.write_series(large)
        self.assertRegex(
            str(raised.exception),
            r"^no block of the pool holds a message or a frame: block of \d+ bytes is "
            r"above the largest block of 57344 bytes$",
        )
        self.assertIsNone(writer.write_series({"plant.a": floats, "plant.time": stamps}))

    def test_open_writer_raises_for_each_argument_that_it_does_not_take(self) -> None:
        cases = [
            ([], {}, ValueError, "the writer names no channel"),
            ([5], {}, TypeError, "a channel name is a str, not int"),
            (
                "plant.a",
                {"authority": 256},
                ValueError,
                "authority is 256, outside 0 to 255",
            ),
            (
                "plant.a",
                {"authority": -1},
                ValueError,
                "authority is -1, outside 0 to 255",
            ),
            (
                "plant.a",
                {"lease": 0.0},
                ValueError,
                "lease is 0.0 s, not a finite number of seconds over 0",
            ),
            (
                "plant.a",
                {"lease": 1e-12},
                ValueError,
                "lease is 1e-12 s, not a finite number of seconds over 0",
            ),
            (
                "plant.a",
                {"lease": float("inf")},
                ValueError,
                "lease is inf s, not a finite number of seconds over 0",
            ),
            (
                "plant.a",
                {"lease": float("nan")},
                ValueError,
                "lease is NaN s, not a finite number of seconds over 0",
            ),
            (
                "plant.a",
                {"on_loss": "drop"},
                ValueError,
                'on_loss is "drop", not "warn", "raise", or "ignore"',
            ),
        ]
        for channels, arguments, kind, text in cases:
            with self.subTest(channels=channels, arguments=arguments):
                with self.assertRaises(kind) as raised:
                    self.client.open_writer(channels, **arguments)
                self.assertIs(type(raised.exception), kind)
                self.assertEqual(str(raised.exception), text)
        with self.assertRaises(ValueError) as raised:
            self.client.open_writer("plant a")
        self.assertRegex(str(raised.exception), r"^channels: ")
        with self.assertRaises(TypeError):
            self.client.open_writer(5)
        for authority, kind in [("1", "str"), (None, "NoneType")]:
            with self.subTest(authority=authority):
                with self.assertRaises(TypeError) as raised:
                    self.client.open_writer("plant.a", authority=authority)
                text = f"authority is an int, not {kind}"
                self.assertEqual(str(raised.exception), text)
        writer = self.client.open_writer(("plant.a",), authority=255, lease=5.0)
        self.assertIsNone(writer.close())

    def test_open_writer_raises_stopped_with_the_code_of_the_node(self) -> None:
        with self.assertRaises(foundation.Stopped) as raised:
            self.client.open_writer(["plant.a", "plant.z"])
        self.assertNotIsInstance(raised.exception, ConnectionError)
        self.assertEqual(raised.exception.code, "unknown")
        self.assertEqual(
            str(raised.exception), f"{UNKNOWN}: the open names plant.a, plant.z"
        )
        with self.assertRaises(foundation.Stopped) as raised:
            self.client.open_writer(["plant.a", "other.a"])
        self.assertEqual(raised.exception.code, "malformed")
        self.assertIsNone(self.client.open_writer("plant.a").close())

    def test_a_writer_of_a_closed_client_raises(self) -> None:
        writer = self.client.open_writer("plant.a")
        self.client.close()
        with self.assertRaises(RuntimeError) as raised:
            self.client.open_writer("plant.a")
        self.assertEqual(str(raised.exception), "the client is closed")
        with self.assertRaises(RuntimeError) as raised:
            writer.write(1.0)
        self.assertEqual(str(raised.exception), "the client is closed")

    def test_a_closed_writer_raises_and_closes_again_with_none(self) -> None:
        with self.client.open_writer("plant.a") as writer:
            writer.write(1.0)
        self.assertIsNone(writer.close())
        for call in (lambda: writer.write(1.0), lambda: writer.write_series({})):
            with self.assertRaises(RuntimeError) as raised:
                call()
            self.assertEqual(str(raised.exception), "the writer is closed")

    def test_two_threads_that_write_on_one_writer_both_finish(self) -> None:
        writer = self.client.open_writer("plant.a")
        self.addCleanup(writer.close)
        raised = []

        def write() -> None:
            try:
                for value in range(300):
                    writer.write(float(value))
            except BaseException as error:
                raised.append(error)

        threads = [threading.Thread(target=write, daemon=True) for _ in range(2)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(PATIENCE)
            self.assertFalse(thread.is_alive())
        self.assertEqual(raised, [])

    def test_other_threads_run_while_a_write_waits(self) -> None:
        with self.client.open_writer("plant.a") as writer:
            self.node.pause()
            # Continues the node when a write that keeps the GIL stops each thread.
            backstop = subprocess.Popen(
                ["sh", "-c", f"sleep 10; kill -CONT {self.node.process.pid}"]
            )
            self.addCleanup(backstop.wait)
            self.addCleanup(backstop.kill)
            counts = []

            def counter() -> None:
                deadline = time.monotonic() + 0.2
                count = 0
                while time.monotonic() < deadline:
                    count += 1
                    time.sleep(0)
                counts.append(count)
                self.node.resume()

            thread = threading.Thread(target=counter, daemon=True)
            thread.start()
            self.assertIsNone(writer.write(1.5))
            thread.join(PATIENCE)
            self.assertGreater(counts[0], 20)

    def test_a_forked_child_does_not_own_the_writer(self) -> None:
        writer = self.client.open_writer("plant.a")
        self.addCleanup(writer.close)
        parent = os.getpid()

        def child() -> object:
            try:
                writer.write(1.0)
            except RuntimeError as error:
                text = (
                    f"the client connected in process {parent}, and this is process "
                    f"{os.getpid()}: connect again in this process"
                )
                if str(error) == text and writer.close() is None:
                    return None
            return "the child wrote"

        self.assertEqual(fork(child), 0)
        self.assertIsNone(writer.write(2.0))

    def test_a_lost_frame_raises_warns_or_passes_as_on_loss_says(self) -> None:
        random = np.random.default_rng(3320)
        count = 3000
        frames = [0]
        now = time.time_ns()

        def series() -> dict:
            start = now + frames[0] * count
            frames[0] += 1
            return {
                "plant.a": random.random(count),
                "plant.time": np.arange(start, start + count, dtype=np.int64),
            }

        writer = self.client.open_writer("plant.a", on_loss="raise")
        self.addCleanup(writer.close)
        for _ in range(2000):
            try:
                writer.write_series(series())
            except foundation.DataLost as error:
                lost = error
                break
        else:
            self.fail("2000 frames of 48 kB lost no sample")
        seqs = lost.seqs
        self.assertEqual(list(seqs), ["plant.a", "plant.time"])
        self.assertEqual(len(seqs["plant.a"]), count)
        self.assertEqual(seqs["plant.a"], seqs["plant.time"])
        self.assertEqual(
            str(lost), f"samples were lost: plant.a ({count}), plant.time ({count})"
        )
        self.assertIsNone(writer.close())
        writer = self.client.open_writer("plant.a", on_loss="warn")
        with self.assertLogs("foundation", "WARNING") as logs:
            self.assertIsNone(writer.write_series(series()))
        self.assertEqual(
            logs.output,
            [f"WARNING:foundation:samples were lost: plant.a ({count}), plant.time "
             f"({count})"],
        )
        self.assertIsNone(writer.close())
        writer = self.client.open_writer("plant.a", on_loss="ignore")
        self.assertIsNone(writer.write_series(series()))
        self.assertIsNone(writer.close())


if __name__ == "__main__":
    unittest.main()
