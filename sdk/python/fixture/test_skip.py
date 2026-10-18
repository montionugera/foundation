import unittest


class SkipTest(unittest.TestCase):
    @unittest.skip("the fixture of a file whose tests all skip")
    def test_skips(self) -> None:
        self.fail("this test ran")
