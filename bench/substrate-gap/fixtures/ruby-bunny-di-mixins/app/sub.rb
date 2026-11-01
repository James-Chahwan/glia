require "bunny"

ch = Bunny.new.tap(&:start).create_channel
q = ch.queue("", exclusive: true)
q.bind("logs")
q.subscribe(block: true) { |_info, _props, body| puts body }
