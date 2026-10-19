"""The test-only functions of a build with the feature `sim`."""

import time
import unittest

import numpy as np

from foundation import _native
from test_client import Node
from test_writer import planted


class SimTest(unittest.TestCase):
    def test_a_panic_on_the_callers_thread_raises_panic_exception(self) -> None:
        with self.assertRaises(BaseException) as raised:
            _native._panic()
        self.assertEqual(type(raised.exception).__name__, "PanicException")
        self.assertEqual(str(raised.exception), "the test panicked")

    def test_a_panic_of_the_clients_thread_raises_panic_exception(self) -> None:
        with self.assertRaises(BaseException) as raised:
            _native._error("panicked")
        self.assertEqual(type(raised.exception).__name__, "PanicException")
        self.assertEqual(str(raised.exception), "the client thread panicked")

    def test_a_thread_that_cannot_start_raises_runtime_error(self) -> None:
        with self.assertRaises(RuntimeError) as raised:
            _native._error("thread")
        self.assertEqual(
            str(raised.exception), "cannot start the client thread: the test"
        )

    def test_an_unknown_kind_is_refused(self) -> None:
        with self.assertRaises(ValueError) as raised:
            _native._error("bind")
        self.assertEqual(str(raised.exception), 'no error "bind"')


class WriterSimTest(unittest.TestCase):
    def test_a_call_that_raises_keeps_the_buffers_for_the_next_call(self) -> None:
        node = Node()
        self.addCleanup(node.stop)
        client = planted(node)
        self.addCleanup(client.close)
        floats = np.zeros(1000)
        stamps = np.arange(1000, dtype=np.int64) + time.time_ns()
        with client.open_writer("plant.a") as writer:
            self.assertEqual((writer._kept, writer._part), (0, 0))
            writer.write_series({"plant.a": floats, "plant.time": stamps})
            kept = writer._kept
            self.assertGreaterEqual(kept, 16000)
            with self.assertRaises(TypeError):
                writer.write_series({"plant.a": floats, "plant.time": floats})
            self.assertEqual(writer._kept, kept)
            with self.assertRaises(TypeError):
                writer.write({"plant.a": "x"})
            self.assertEqual(writer._kept, kept)
            self.assertGreater(writer._part, 0)


if __name__ == "__main__":
    unittest.main()
