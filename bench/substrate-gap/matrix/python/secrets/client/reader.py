import os

import stripe

stripe.api_key = os.environ["STRIPE_SECRET_KEY"]


def charge(amount_cents):
    return stripe.PaymentIntent.create(amount=amount_cents, currency="usd")
