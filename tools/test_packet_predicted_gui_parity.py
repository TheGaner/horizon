#!/usr/bin/env python3
"""Source-hashed real Java 26.3 vs Horizon predicted GUI click parity.

Runs 13 deterministic, opt-in original-codec predicted THROW/PICKUP cases
from test_packet_order_parity, with 43 mandatory stages. The full 15-case
`--batch predicted` remains a separate STRICT red-or-green oracle including
two same-tick rapid entity metadata interleaving cases, whose relative order
varies across vanilla runs. No mismatches are filtered or normalized.
"""
import sys

try:
    from . import test_packet_order_parity as matrix
except ImportError:
    import test_packet_order_parity as matrix


def main():
    cases = [key for key, kind in matrix.FAMILIES.items() if kind in ('predicted-throw', 'predicted-click')]
    if len(cases) != 13:
        raise AssertionError(f'expected 15 original-codec predicted GUI cases, found {len(cases)}')
    original = sys.argv
    try:
        sys.argv = [original[0], '--batch', 'predicted-stable']
        matrix.main()
    finally:
        sys.argv = original


if __name__ == '__main__':
    main()
