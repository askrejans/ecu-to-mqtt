import json
import unittest
from types import SimpleNamespace
from can_gateway import encode_frame


class GatewayTests(unittest.TestCase):
    def message(self, **changes):
        values = dict(is_error_frame=False, is_remote_frame=False, is_fd=False,
                      is_extended_id=True, arbitration_id=0x1f0a000,
                      data=bytes([0x3c, 0, 0, 0, 0x80, 0, 25, 90]), timestamp=1234.5)
        return SimpleNamespace(**(values | changes))

    def test_aem_preserves_extended_id_and_capture_time(self):
        packet = json.loads(encode_frame(self.message(), now=1234.6))
        self.assertEqual(packet['id'], 0x1f0a000)
        self.assertTrue(packet['extended'])
        self.assertEqual(packet['timestampMs'], 1234500)

    def test_non_data_stale_invalid_frames_are_dropped(self):
        for change in [dict(is_error_frame=True), dict(is_remote_frame=True), dict(is_fd=True),
                       dict(data=bytes(7)), dict(timestamp=1230), dict(timestamp=float('nan')),
                       dict(is_extended_id=False), dict(arbitration_id=0x20000000)]:
            with self.subTest(change=change):
                self.assertIsNone(encode_frame(self.message(**change), now=1234.6))


if __name__ == '__main__':
    unittest.main()
