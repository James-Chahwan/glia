import ldclient

client = ldclient.get()
BANNER = client.variation("promo-banner", None, False)


def checkout(user):
    if client.variation("new-checkout", user, False):
        return 1
    return 0
