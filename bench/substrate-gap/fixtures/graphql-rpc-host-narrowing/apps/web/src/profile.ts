import { gql, useQuery } from "@apollo/client";
import { api } from "./trpc";

const GET_USER = gql`
  query getUser {
    getUser
  }
`;

export function Profile() {
  const items = api.item.list.useQuery();
  return useQuery(GET_USER);
}
