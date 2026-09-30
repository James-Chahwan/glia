require "nats/client"

nc = NATS.connect("nats://localhost:4222")
nc.subscribe("orders") { |msg| puts msg.data }
