from signals import order_placed


@order_placed.connect
def send_receipt(sender, **kw):
    print("receipt", kw)


def place(order):
    order_placed.send("orders", order=order)
