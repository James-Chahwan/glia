// graphql-request client: the real GraphQL operation in this fixture.
import { request, gql } from "graphql-request";

const LIST_USERS = gql`
  query ListUsers {
    users { id name }
  }
`;

export async function listUsers(endpoint: string) {
  return request(endpoint, LIST_USERS);
}
