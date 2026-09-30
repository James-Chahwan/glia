require "bunny"

conn = Bunny.new
conn.start
ch = conn.create_channel
ch.default_exchange.publish("order-1", routing_key: "orders")
