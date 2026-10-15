require "dry/container"
require "dry/auto_inject"
require_relative "user_service"

class Container
  extend Dry::Container::Mixin

  register("user_service") { UserService.new }
end

Import = Dry::AutoInject(Container)
