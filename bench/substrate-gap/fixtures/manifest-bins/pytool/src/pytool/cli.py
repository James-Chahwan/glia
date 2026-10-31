import sys


def main() -> int:
    print("pytool", sys.argv[1:])
    return 0


class Sync:
    @staticmethod
    def run() -> int:
        return main()
