#!/usr/bin/env python3
"""Pure-Python correctness checks for the live packet-order differential analyzer."""
import unittest

try:
    from . import test_packet_order_parity as matrix
except ImportError:
    import test_packet_order_parity as matrix


class PacketOrderComparatorTests(unittest.TestCase):
    def setUp(self):
        self.case = next(k for k in matrix.FAMILIES if k == 'gui-single-stick-8')
        self.prefix = f'Alice step={self.case} '

    def check(self, a, b):
        return matrix.assess([self.prefix + item for item in a],
                             [self.prefix + item for item in b])

    def test_exact_stream(self):
        result = self.check(['item-added', 'item-data minecraft:stick x1', 'slot 0 36 minecraft:stick x7'],
                            ['item-added', 'item-data minecraft:stick x1', 'slot 0 36 minecraft:stick x7'])
        self.assertEqual(result['strict_equal_receiver_case_streams'], 1)
        self.assertFalse(result['ordering_only'])
        self.assertFalse(result['content_or_count'])

    def test_reversed_sequence_is_reported_as_order_not_content(self):
        result = self.check(['item-added', 'slot 0 36 empty'], ['slot 0 36 empty', 'item-added'])
        self.assertEqual(len(result['ordering_only']), 1)
        self.assertFalse(result['content_or_count'])
        self.assertEqual(result['ordering_only'][0]['case'], self.case)

    def test_missing_packet_is_not_normalized(self):
        result = self.check(['item-added', 'item-data minecraft:stick x1'], ['item-added'])
        self.assertEqual(len(result['content_or_count']), 1)
        self.assertFalse(result['ordering_only'])

    def test_wrong_item_count_is_not_normalized(self):
        result = self.check(['item-data minecraft:stick x1'], ['item-data minecraft:stick x8'])
        self.assertEqual(len(result['content_or_count']), 1)

    def test_receiver_streams_not_globally_interleaved(self):
        raw_a = [self.prefix + 'item-added', f'Bob step={self.case} item-added',
                 self.prefix + 'item-data minecraft:stick x1']
        raw_b = [f'Bob step={self.case} item-added', self.prefix + 'item-added',
                 self.prefix + 'item-data minecraft:stick x1']
        result = matrix.assess(raw_a, raw_b)
        self.assertEqual(result['strict_equal_receiver_case_streams'], 2)
        self.assertFalse(result['ordering_only'])
        self.assertFalse(result['content_or_count'])

    def test_protocol_error_is_never_ignored(self):
        result = matrix.assess([self.prefix + 'item-added'],
                               [self.prefix + 'item-added', self.prefix + 'error play java.lang.Exception'])
        self.assertEqual(len(result['errors_horizon']), 1)

    def test_action_debug_lines_are_not_clientbound_packets(self):
        result = self.check(['sent gui single button=0','item-added'],['item-added'])
        self.assertEqual(result['strict_equal_receiver_case_streams'], 1)
        self.assertEqual(result['observed_packets_vanilla'], 1)


if __name__ == '__main__':
    unittest.main()
