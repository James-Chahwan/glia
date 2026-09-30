class OrdersConsumer < Karafka::BaseConsumer
  def consume
    messages.each { |m| puts m.payload }
  end
end
