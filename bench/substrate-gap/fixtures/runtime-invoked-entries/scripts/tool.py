import sys


def compute(n):
    def double(x):
        return x * 2

    def unused_inner():
        return 0

    return double(n)


def double(x):
    return x + x


def helper():
    return 1


def init():
    return 0


if __name__ == "__main__":
    def later():
        helper()

    print(compute(int(sys.argv[1])))
