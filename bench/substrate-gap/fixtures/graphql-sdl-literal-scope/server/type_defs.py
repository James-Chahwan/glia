# Ariadne schema-first: SDL in a gql("""...""") literal.
from ariadne import gql

type_defs = gql("""
    type Mutation {
        placeOrder(sku: String!): Order
    }
""")
