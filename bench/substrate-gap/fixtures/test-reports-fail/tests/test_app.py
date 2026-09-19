from api.app import list_orders

def test_list_orders():
    assert list_orders() == []
