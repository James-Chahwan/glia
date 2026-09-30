import { gql, useQuery } from "@apollo/client";

const ALL_USERS = gql`
  query AllUsers {
    allUsers {
      id
    }
  }
`;

export function useAllUsers() {
  return useQuery(ALL_USERS);
}
