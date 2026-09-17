import nats


async def publish_order(payload):
    nc = await nats.connect("nats://localhost:4222")
    await nc.publish("orders", payload)
    await nc.drain()
