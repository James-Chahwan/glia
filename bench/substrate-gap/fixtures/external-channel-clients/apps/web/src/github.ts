import { ApolloClient, InMemoryCache, gql } from "@apollo/client";

export const gh = new ApolloClient({ uri: "https://api.github.com/graphql", cache: new InMemoryCache() });

export function viewer() {
  return gh.query({ query: gql`query GetUser { getUser { login } }` });
}

export function ticker(): WebSocket {
  return new WebSocket("wss://stream.binance.com/ws");
}

export function feed(): WebSocket {
  return new WebSocket("ws://localhost:8080/feed");
}
