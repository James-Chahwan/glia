from UnleashClient import UnleashClient

client = UnleashClient(url="https://unleash.internal/api", app_name="checkout")
client.initialize_client()


def checkout(cart):
    if client.is_enabled("new_checkout"):
        return {"flow": "new", "cart": cart}
    return {"flow": "legacy", "cart": cart}
