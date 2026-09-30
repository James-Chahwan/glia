class OrderPublisher
  def publish(order)
    Karafka.producer.produce_async(topic: "orders", payload: order.to_json)
  end
end
