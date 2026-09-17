import { ApolloClient, InMemoryCache, gql } from "@apollo/client";

const client = new ApolloClient({ uri: "http://api/graphql", cache: new InMemoryCache() });

const USER = gql`
  query User($id: ID!) {
    user(id: $id) {
      name
    }
  }
`;

export async function fetchUser(id: string) {
  const { data } = await client.query({ query: USER, variables: { id } });
  return data.user;
}
