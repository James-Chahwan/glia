import { ApolloClient, InMemoryCache, gql } from '@apollo/client/core';
import { createClient } from 'graphql-ws';

export const client = new ApolloClient({
  uri: 'http://localhost:8080/graphql',
  cache: new InMemoryCache(),
});

export const wsClient = createClient({ url: 'ws://localhost:8080/subscriptions' });

export const PRODUCTS = gql`
  query Products {
    products {
      id
    }
  }
`;
