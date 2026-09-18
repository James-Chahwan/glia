// Apollo Server schema-first: the SDL lives in a gql-tagged template.
import { gql } from "graphql-tag";

export const typeDefs = gql`
  type Query {
    listOrders: [Order]
  }

  type Order {
    id: ID!
  }
`;
