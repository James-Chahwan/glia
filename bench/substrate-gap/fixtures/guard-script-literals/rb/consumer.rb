require 'karafka'

# topic "ghost_rb"
# topic :ghost_rb do consumer GhostConsumer end
class App < Karafka::App
  routes.draw do
    topic :orders do
      consumer OrdersConsumer
    end
  end
end

class OrdersConsumer < Karafka::BaseConsumer
  def consume
    messages.each { |m| puts m.payload }
  end
end
