import { ApolloClient, InMemoryCache } from '@apollo/client/core';

export const client = new ApolloClient({
  uri: 'http://localhost:8080/graphql',
  cache: new InMemoryCache(),
});
