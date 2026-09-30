class Orders
  def place(order)
    ActiveSupport::Notifications.instrument("order.placed", order: order)
  end
end

ActiveSupport::Notifications.subscribe("order.placed") do |event|
  Rails.logger.info(event.payload)
end
