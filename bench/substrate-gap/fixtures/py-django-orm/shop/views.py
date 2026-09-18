from .models import Order


def recent_orders():
    return Order.objects.filter(total__gt=0)
