import { ApolloClient, InMemoryCache } from "@apollo/client";

export const client = new ApolloClient({ uri: "http://users-svc/graphql", cache: new InMemoryCache() });
