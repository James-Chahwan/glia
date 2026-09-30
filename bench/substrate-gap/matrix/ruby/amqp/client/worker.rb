require "bunny"

conn = Bunny.new
conn.start
ch = conn.create_channel
q = ch.queue("orders", durable: true)
q.subscribe(block: true) do |_delivery, _props, body|
  puts body
end
