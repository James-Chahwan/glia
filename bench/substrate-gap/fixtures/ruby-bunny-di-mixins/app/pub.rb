require "bunny"

ch = Bunny.new.tap(&:start).create_channel
x = ch.fanout("logs")
x.publish("m")
