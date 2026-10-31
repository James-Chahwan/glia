import time


def process(item):
    return item * 2


def loop():
    while True:
        process(1)
        time.sleep(1)


loop()
