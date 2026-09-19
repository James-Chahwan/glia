from shop.orders.service import price


def test_price():
    assert price({"items": [{"p": 2}]}) == 2
