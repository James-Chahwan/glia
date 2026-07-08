"""Ordering domain — the symbols the Confluence fixtures reference by name."""


class OrderService:
    """Validates and persists orders; charges via the payment gateway."""

    def place_order(self, cart):
        gateway = PaymentGateway()
        return gateway.charge(cart.total)


class PaymentGateway:
    def charge(self, amount):
        # Pretend to talk to a PSP.
        return {"captured": amount}
