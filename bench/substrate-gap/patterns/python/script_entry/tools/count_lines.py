#!/usr/bin/env python3
# A shell script: run as `count_lines.py <file>`.
import sys


def count(path):
    with open(path) as handle:
        return sum(1 for _ in handle)


if __name__ == "__main__":
    print(count(sys.argv[1]))
