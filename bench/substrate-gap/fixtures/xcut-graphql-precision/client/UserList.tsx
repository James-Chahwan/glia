// Un-named Apollo calls: GET_USERS is imported, so there is no gql template in
// this file and both operation names fall back to the needle text.
import { useApolloClient, useQuery } from "@apollo/client";
import { GET_USERS } from "./queries";

export function UserList() {
  const { data } = useQuery(GET_USERS);
  return data;
}

export async function refreshUsers() {
  const client = useApolloClient();
  const res = await client.query({ query: GET_USERS });
  return res.data;
}
