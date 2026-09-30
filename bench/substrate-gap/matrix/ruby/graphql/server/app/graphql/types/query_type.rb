module Types
  class QueryType < Types::BaseObject
    field :orders, [Types::OrderType], null: false

    def orders
      Order.all
    end
  end
end
