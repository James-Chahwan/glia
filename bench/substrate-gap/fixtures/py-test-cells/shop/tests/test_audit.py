from shop.orders.audit import audited_place


def test_audited_place():
    assert audited_place({"items": []})["total"] == 0
