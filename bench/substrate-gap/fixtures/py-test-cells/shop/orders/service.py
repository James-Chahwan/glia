def price(order):
    return sum(i["p"] for i in order["items"])


def place(order):
    total = price(order)
    return {"total": total}
