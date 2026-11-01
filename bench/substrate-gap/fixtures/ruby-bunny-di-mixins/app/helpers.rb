module Helpers
  def content_type(type) = type
end

module Routing
  def route(path) = path
end

class Base
  include Helpers
  extend Routing

  route "/home"

  def call!(env) = content_type(env)
end
