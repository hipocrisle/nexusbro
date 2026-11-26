#!/usr/bin/env python3
"""Pure metadata helpers for the initial address-entry receipt (no OS deps).

Only bounded lengths, a fixed match class and a bounded first-mismatch index
ever leave these functions — never the raw string, a hash, a URL or a secret.
"""

MAX_VALUE_LENGTH = 4096
MATCH_CLASSES = ('empty', 'exact', 'prefix', 'contains', 'other')


def bounded_length(value):
    try:
        return min(len(value), MAX_VALUE_LENGTH)
    except TypeError:
        return 0


def first_mismatch(actual, expected):
    """Bounded index of the first differing character, or None when equal."""
    if not isinstance(actual, str) or not isinstance(expected, str):
        return None
    limit = min(len(actual), len(expected), MAX_VALUE_LENGTH)
    for index in range(limit):
        if actual[index] != expected[index]:
            return index
    if len(actual) != len(expected):
        return limit
    return None


def classify_value(actual, expected):
    """SAFE receipt of a failed/matched comparison: length + class + bounded
    first mismatch. The actual string itself is never returned."""
    if not isinstance(actual, str):
        actual = ''
    length = bounded_length(actual)
    if actual == expected:
        match = 'exact'
    elif actual == '':
        match = 'empty'
    elif expected.startswith(actual) and actual:
        match = 'prefix'
    elif actual and actual in expected:
        match = 'contains'
    else:
        match = 'other'
    mismatch = first_mismatch(actual, expected)
    return {'actual_length': length,
            'match_class': match if match in MATCH_CLASSES else 'other',
            'first_mismatch': mismatch if isinstance(mismatch, int) and 0 <= mismatch <= MAX_VALUE_LENGTH else None}
