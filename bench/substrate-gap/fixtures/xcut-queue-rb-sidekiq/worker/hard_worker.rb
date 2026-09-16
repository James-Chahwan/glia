# Consumer side: the Sidekiq worker. No literal names the task anywhere —
# the identity is the ENCLOSING CLASS.
class HardWorker
  include Sidekiq::Worker
  sidekiq_options queue: 'critical', retry: 3

  def perform(order_id)
    Order.find(order_id).fulfil!
  end
end
