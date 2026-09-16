# Producer side: a Rails controller that enqueues a Sidekiq job.
class OrdersController < ApplicationController
  def create
    order = Order.create!(params[:order])
    # The receiver IS the task identity; `order.id` is the payload.
    HardWorker.perform_async(order.id)
    head :accepted
  end
end
