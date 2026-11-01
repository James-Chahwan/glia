import * as Apollo from '@apollo/client';

export const nsClient = new Apollo.ApolloClient({
  uri: '/ns/graphql',
  cache: new Apollo.InMemoryCache(),
});
