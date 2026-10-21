#!/usr/bin/env python3
"""Deterministic unit checks for the per-entity original-codec parity analyzer."""
import unittest

try:
    from . import test_packet_entity_lifecycle_parity as life
except ImportError:
    import test_packet_entity_lifecycle_parity as life


class LifecycleAnalysisTests(unittest.TestCase):
    def test_valid_lifetime_with_metadata_and_removal(self):
        traces = {
            ('Alice', 'case-spawn'): ['entity-spawn #11', 'entity-meta #11 minecraft:stick x8'],
            ('Alice', 'case-settle'): ['entity-meta #11 minecraft:stick x8'],
            ('Alice', 'case-remove'): ['entity-remove #11'],
        }
        result, problems = life.normalized_lifetimes(traces, 'Alice','case')
        self.assertFalse(problems)
        self.assertEqual(len(result),4)
        self.assertEqual({x[2] for x in result}, {'#1'})

    def test_different_numeric_ids_normalize_per_receiver(self):
        def trace(num):
            return {('Alice','case-spawn'):[f'entity-spawn #{num}',f'entity-meta #{num} minecraft:stick x2'],
                    ('Alice','case-remove'):[f'entity-remove #{num}']}
        self.assertEqual(life.normalized_lifetimes(trace(11),'Alice','case'),
                         life.normalized_lifetimes(trace(999),'Alice','case'))

    def test_unknown_metadata_target_is_error(self):
        _,issues=life.normalized_lifetimes({('Bob','case-spawn'): ['entity-meta #41 minecraft:stick x1']},'Bob','case')
        self.assertTrue(any('unknown-target' in issue for issue in issues))

    def test_missing_remove_is_error(self):
        _,issues=life.normalized_lifetimes({('Alice','case-spawn'):[
            'entity-spawn #1','entity-meta #1 minecraft:stick x1']},'Alice','case')
        self.assertTrue(any('missing-entity-remove' in issue for issue in issues))

    def test_missing_metadata_is_error(self):
        _,issues=life.normalized_lifetimes({('Alice','case-spawn'):['entity-spawn #1'],
                                             ('Alice','case-remove'):['entity-remove #1']},'Alice','case')
        self.assertTrue(any('missing-item-metadata' in issue for issue in issues))

    def test_duplicate_spawn_is_error(self):
        _,issues=life.normalized_lifetimes({('Alice','case-spawn'):[
            'entity-spawn #1','entity-spawn #1','entity-meta #1 minecraft:stick x1'],
            ('Alice','case-remove'):['entity-remove #1']},'Alice','case')
        self.assertTrue(any('duplicate-spawn' in issue for issue in issues))

    def test_both_receivers_are_separate(self):
        lines=['Alice step=case-spawn entity-spawn #1',
               'Bob step=case-spawn entity-spawn #99',
               'Alice step=case-spawn entity-meta #1 minecraft:stick x1']
        groups,errors=life.parse(lines)
        self.assertFalse(errors)
        self.assertEqual(len(groups[('Alice','case-spawn')]),2)
        self.assertEqual(len(groups[('Bob','case-spawn')]),1)

if __name__=='__main__':unittest.main()
