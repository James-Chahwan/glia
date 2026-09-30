from django.dispatch import receiver

from signals import order_placed


@receiver(order_placed)
def send_receipt(sender, **kwargs):
    print("receipt")


def place(order):
    order_placed.send(sender=None, order=order)
