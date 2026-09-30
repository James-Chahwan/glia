import { ApolloClient, InMemoryCache, gql } from "@apollo/client";

const client = new ApolloClient({ uri: "http://api/graphql", cache: new InMemoryCache() });

const ORDERS = gql`
  query Orders {
    orders {
      id
    }
  }
`;

export async function listOrders() {
  const { data } = await client.query({ query: ORDERS });
  return data.orders;
}
