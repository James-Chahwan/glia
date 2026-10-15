require_relative "container"

class OrderController
  include Import["user_service"]

  def show(id)
    user_service.find(id)
  end
end
