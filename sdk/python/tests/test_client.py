"""The Python SDK against a node process.

`FOUNDATION_NODE` is the path of the `foundation` binary that each test starts.
"""

import gc
import ipaddress
import os
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import uuid
import warnings

import foundation

PATIENCE = 90.0
# The node sends its close before it exits. No call of the client shows that the client
# took it, so a test gives the client 1 s on loopback. It goes with item 1 of #2953.
CLOSED = 1.0
ADMIN = "@admin"
# The encoding of the base point of Ed25519: a public key that is not of small order.
BASE = bytes([0x58]) + bytes([0x66] * 31)
WRONG = bytes(31) + b"\x01"


def crc32c(data: bytes) -> int:
    crc = 0xFFFFFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ (0x82F63B78 & -(crc & 1))
    return crc ^ 0xFFFFFFFF


def budget(pool: int, disk: int) -> bytes:
    """The budget file of a node: the tag, `pool` and `disk`, and the CRC32C."""
    body = b"foundation/budget/1" + pool.to_bytes(8, "little")
    body += disk.to_bytes(8, "little")
    return body + crc32c(body).to_bytes(4, "little")


def fork(call) -> int:
    """Calls `call` in a forked child, and gives the exit code of the child: 0 when
    `call` gives `None`. Kills the child after `PATIENCE` s."""
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        pid = os.fork()
    if pid == 0:
        code = 1
        try:
            code = 0 if call() is None else 1
        finally:
            os._exit(code)
    deadline = time.monotonic() + PATIENCE
    while True:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            return os.waitstatus_to_exitcode(status)
        if time.monotonic() > deadline:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
            raise AssertionError(f"the child {pid} did not exit")
        time.sleep(0.01)


class Node:
    """`foundation start --name edge` in a new directory, with a budget of 1 GiB of
    pool and 8 MiB of disk for each core. It goes with #3064."""

    def __init__(self) -> None:
        self.dir = tempfile.TemporaryDirectory(prefix="foundation-python-")
        data = os.path.join(self.dir.name, "foundation-data", "data")
        os.makedirs(data)
        with open(os.path.join(data, "budget"), "wb") as file:
            file.write(budget(1 << 30, os.cpu_count() * (8 << 20)))
        self.data = data
        self.process = subprocess.Popen(
            [os.environ["FOUNDATION_NODE"], "start", "--name", "edge"],
            cwd=self.dir.name,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        line = self.process.stdout.readline()
        if not line:
            self.process.wait(PATIENCE)
            raise AssertionError(
                f"the node exited with {self.process.returncode}:\n"
                + self.process.stderr.read().decode()
            )
        address = self.read("address", 99, b"foundation/address/1")
        admin = self.read("admin.key", 54, b"foundation/admin/1")
        family = address[68]
        if family == 4:
            ip = ipaddress.IPv4Address(address[69:73])
        elif family == 6:
            ip = ipaddress.IPv6Address(address[69:85])
        else:
            raise AssertionError(f"the address has family {family}")
        if ip.is_unspecified:
            ip = ipaddress.ip_address("127.0.0.1" if family == 4 else "::1")
        port = int.from_bytes(address[85:87], "big")
        self.address = f"{ip}:{port}" if family == 4 else f"[{ip}]:{port}"
        self.key = uuid.UUID(bytes=address[20:36])
        self.public_key = address[36:68]
        self.admin = admin[18:50]

    def read(self, name: str, length: int, tag: bytes) -> bytes:
        """The bytes of the file `name` of the data directory, once it holds `tag`.
        Checks the CRC32C of the file."""
        path = os.path.join(self.data, name)
        deadline = time.monotonic() + PATIENCE
        while True:
            try:
                with open(path, "rb") as file:
                    data = file.read()
            except FileNotFoundError:
                data = b""
            if len(data) == length and data.startswith(tag):
                body = length - 4
                crc = crc32c(data[:body]).to_bytes(4, "little")
                if crc != data[body:]:
                    raise AssertionError(f"the CRC32C of {name}")
                return data
            if time.monotonic() > deadline:
                raise AssertionError(f"the node writes no {name}: {data!r}")
            time.sleep(0.01)

    def connect(self, **arguments) -> foundation.Client:
        """`foundation.connect` to this node as its admin, with `arguments` in place of
        the node's own."""
        return foundation.connect(
            **{
                "address": self.address,
                "node": self.key,
                "public_key": self.public_key,
                "subject": ADMIN,
                "private_key": self.admin,
                **arguments,
            }
        )

    def pause(self) -> None:
        """Stops the node with SIGSTOP, and returns once it stopped: on macOS, it can
        run on after `kill` returns. `stop` continues it."""
        self.process.send_signal(signal.SIGSTOP)
        os.waitpid(self.process.pid, os.WUNTRACED)

    def stop(self) -> None:
        """Stops the node with SIGTERM, and waits until it exits."""
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGCONT)
            self.process.send_signal(signal.SIGTERM)
        self.process.communicate(timeout=PATIENCE)
        self.dir.cleanup()


class ClientTest(unittest.TestCase):
    def setUp(self) -> None:
        self.node = Node()
        self.addCleanup(self.node.stop)

    def test_a_client_connects_and_connects_again_after_its_close(self) -> None:
        client = self.node.connect()
        self.assertIsNone(client.close())
        self.assertIsNone(client.close())
        client = self.node.connect()
        del client

    def test_a_with_block_closes_the_client(self) -> None:
        with self.node.connect() as client:
            self.assertIsInstance(client, foundation.Client)
        self.node.stop()
        time.sleep(CLOSED)
        self.assertIsNone(client.close())

    def test_a_raised_block_gets_no_note_when_the_close_succeeds(self) -> None:
        with self.assertRaises(KeyError) as raised:
            with self.node.connect():
                raise KeyError("in the block")
        self.assertFalse(hasattr(raised.exception, "__notes__"))

    def test_two_threads_that_close_one_client_both_return(self) -> None:
        client = self.node.connect()
        threads = [
            threading.Thread(target=client.close, daemon=True) for _ in range(2)
        ]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(PATIENCE)
        self.assertEqual([thread.is_alive() for thread in threads], [False, False])

    def test_other_threads_run_while_a_client_closes(self) -> None:
        client = self.node.connect()
        self.node.pause()
        count = 0
        closed = threading.Event()

        def counter() -> None:
            nonlocal count
            while not closed.is_set():
                count += 1
                # Gives up the GIL with no wait: on macOS a sleep of 1 ms can take
                # 20 ms, and the close of a paused node takes about 0.4 s.
                time.sleep(0)

        thread = threading.Thread(target=counter, daemon=True)
        thread.start()
        try:
            client.close()
        finally:
            closed.set()
            thread.join()
        self.assertGreater(count, 20)

    def test_a_child_forked_during_a_close_gives_none(self) -> None:
        client = self.node.connect()
        self.node.pause()
        closing = threading.Thread(target=client.close, daemon=True)
        closing.start()
        time.sleep(0.02)
        self.assertTrue(closing.is_alive())
        self.assertEqual(fork(client.close), 0)
        closing.join(PATIENCE)
        self.assertFalse(closing.is_alive())

    def test_a_child_forked_with_an_open_client_does_not_end_the_session(self) -> None:
        clients = [self.node.connect()]

        def close_and_drop() -> object:
            raised = []
            sys.unraisablehook = raised.append
            closed = clients[0].close()
            clients.clear()
            gc.collect()
            return raised or closed

        self.assertEqual(fork(close_and_drop), 0)
        self.assertIsNone(clients[0].close())

    def test_a_child_that_drops_an_open_client_touches_no_descriptor(self) -> None:
        clients = [self.node.connect()]

        def drop() -> object:
            raised = []
            sys.unraisablehook = raised.append
            os.closerange(3, 65536)
            clients.clear()
            gc.collect()
            return raised or None

        self.assertEqual(fork(drop), 0)
        self.assertIsNone(clients[0].close())

    def test_a_subject_that_the_spec_does_not_list_is_refused(self) -> None:
        with self.assertRaises(ConnectionError) as raised:
            self.node.connect(subject="plant.operator")
        self.assertEqual(
            str(raised.exception),
            "the spec has no such subject, does not list the key for it, or the "
            "signature is not valid",
        )

    def test_a_wrong_public_key_is_not_authenticated(self) -> None:
        with self.assertRaises(ConnectionError) as raised:
            self.node.connect(public_key=WRONG)
        address = self.node.address
        self.assertEqual(
            str(raised.exception),
            f"the session failed: no address reached peer {WRONG.hex()}; "
            f"Udp({address}): the peer did not prove key {WRONG.hex()}",
        )

    def test_the_close_gives_the_cause_when_the_node_closed_the_session(self) -> None:
        client = self.node.connect()
        self.node.stop()
        time.sleep(CLOSED)
        with self.assertRaises(ConnectionError) as raised:
            client.close()
        self.assertEqual(
            str(raised.exception), "the session failed: the peer closed the session (0)"
        )
        self.assertIsNone(client.close())

    def test_a_with_block_raises_the_close_error_when_it_did_not_raise(self) -> None:
        with self.assertRaises(ConnectionError) as raised:
            with self.node.connect():
                self.node.stop()
                time.sleep(CLOSED)
        self.assertEqual(
            str(raised.exception), "the session failed: the peer closed the session (0)"
        )

    def test_a_with_block_that_raised_gives_the_close_error_as_a_note(self) -> None:
        with self.assertRaises(KeyError) as raised:
            with self.node.connect():
                self.node.stop()
                time.sleep(CLOSED)
                raise KeyError("in the block")
        self.assertEqual(repr(raised.exception), "KeyError('in the block')")
        self.assertEqual(
            raised.exception.__notes__,
            [
                "the close raised ConnectionError: the session failed: the peer closed "
                "the session (0)"
            ],
        )


class ArgumentTest(unittest.TestCase):
    def connect(self, **arguments) -> foundation.Client:
        return foundation.connect(
            **{
                "address": "127.0.0.1:1",
                "node": uuid.UUID(int=1),
                "public_key": BASE,
                "subject": ADMIN,
                "private_key": bytes(32),
                **arguments,
            }
        )

    def test_an_address_that_is_not_ip_and_port_is_refused(self) -> None:
        with self.assertRaises(ValueError) as raised:
            self.connect(address="localhost:1")
        self.assertEqual(
            str(raised.exception),
            "the address \"localhost:1\" is not ip:port: invalid socket address syntax",
        )

    def test_a_public_key_that_is_not_32_bytes_is_refused(self) -> None:
        with self.assertRaises(ValueError) as raised:
            self.connect(public_key=bytes(31))
        self.assertEqual(str(raised.exception), "public_key is 31 bytes, not 32")

    def test_a_public_key_of_small_order_is_refused(self) -> None:
        with self.assertRaises(ValueError) as raised:
            self.connect(public_key=bytes(32))
        self.assertEqual(
            str(raised.exception),
            "public_key: the public key is a point of small order",
        )

    def test_a_private_key_that_is_not_32_bytes_is_refused(self) -> None:
        with self.assertRaises(ValueError) as raised:
            self.connect(private_key=bytes(33))
        self.assertEqual(str(raised.exception), "private_key is 33 bytes, not 32")

    def test_a_node_that_is_not_a_uuid_is_refused(self) -> None:
        with self.assertRaises(TypeError) as raised:
            self.connect(node=1)
        self.assertEqual(str(raised.exception), "node is int, not uuid.UUID")

    def test_a_subject_that_is_not_a_name_is_refused(self) -> None:
        with self.assertRaises(ValueError) as raised:
            self.connect(subject="")
        self.assertEqual(str(raised.exception), "subject: a name or pattern is empty")

    def test_other_threads_run_while_a_client_dials(self) -> None:
        silent = socket.socket(socket.AF_INET6, socket.SOCK_DGRAM)
        self.addCleanup(silent.close)
        silent.bind(("::1", 0))
        port = silent.getsockname()[1]
        count = 0
        dialing = threading.Event()

        def counter() -> None:
            nonlocal count
            while not dialing.is_set():
                count += 1
                time.sleep(0.001)

        thread = threading.Thread(target=counter, daemon=True)
        thread.start()
        try:
            with self.assertRaises(ConnectionError) as raised:
                self.connect(address=f"[::1]:{port}")
        finally:
            dialing.set()
            thread.join()
        self.assertGreater(count, 100)
        self.assertEqual(
            str(raised.exception),
            f"the session failed: no address reached peer {BASE.hex()}; "
            f"Udp([::1]:{port}): the peer stopped answering",
        )


if __name__ == "__main__":
    unittest.main()
