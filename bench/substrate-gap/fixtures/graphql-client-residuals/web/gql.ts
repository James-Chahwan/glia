import { ApolloClient as AC, InMemoryCache, gql } from '@apollo/client';

export type OrdersQuery = { orders: Array<{ id: string }> };

export const client = new AC({ uri: '/graphql', cache: new InMemoryCache() });

export function loadOrders() {
  return client.query<OrdersQuery>({ query: gql`query { orders { id } }` });
}
