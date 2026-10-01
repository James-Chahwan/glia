class Checkout
  def call(order)
    HardWorker.perform_async(order.id)
  end
end
