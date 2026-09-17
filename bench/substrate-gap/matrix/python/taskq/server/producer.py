from tasks import process_order


def checkout(order_id):
    process_order.delay(order_id)
    return {"queued": True}
