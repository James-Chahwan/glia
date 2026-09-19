from shop.orders.service import place


def audited_place(order):
    return place(order)
