require "nats/client"

nc = NATS.connect("nats://localhost:4222")
nc.publish("orders", "order-1")
