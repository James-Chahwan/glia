import nats


async def handle(msg):
    print(msg.data)


async def run():
    nc = await nats.connect("nats://localhost:4222")
    await nc.subscribe("orders", cb=handle)
