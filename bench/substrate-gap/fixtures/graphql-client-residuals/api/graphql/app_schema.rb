class AppSchema < GraphQL::Schema
  query Types::RootQuery
end

module Types
  class RootQuery < Types::BaseObject
    field :invoices, [Types::InvoiceType], null: false

    def invoices
      Invoice.all
    end
  end
end
