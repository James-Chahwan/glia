from shop.base import BaseController


class OrdersController(BaseController):
    def list_orders(self, user_id):
        rows = self.repos["orders"]
        mine = [row for row in rows if row.get("user") == user_id]
        return self.render(mine)

    def render(self, rows):
        self.last_status = 200 if rows else 404
        return {"status": self.last_status, "rows": rows}


class Price:
    def __init__(self, amount):
        self.amount = amount

    @property
    def amount(self):
        return self._amount

    @amount.setter
    def amount(self, value):
        if value < 0:
            raise ValueError("negative price")
        self._amount = value
