from pyee.base import EventEmitter

ee = EventEmitter()


@ee.on("order-placed")
def send_receipt(order):
    print("receipt", order)


def place(order):
    ee.emit("order-placed", order)
