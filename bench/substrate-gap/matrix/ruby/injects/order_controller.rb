require 'dry/container'
require_relative 'user_service'

class OrderController
  def initialize(user_service)
    @user_service = user_service
  end
end
