"""Positive control: intra-file CALLS extraction (Python)."""


def helper(x):
    return x + 1


def compute(n):
    total = 0
    for i in range(n):
        total = helper(total)  # CALLS compute -> helper
    return total
