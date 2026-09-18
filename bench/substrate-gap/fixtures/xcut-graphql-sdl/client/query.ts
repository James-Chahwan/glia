import { gql, useQuery } from '@apollo/client';

const GET_USER = gql`
  query getUser($id: ID!) {
    getUser(id: $id) {
      id
      name
    }
  }
`;

export function UserProfile({ id }: { id: string }) {
  const { data, loading } = useQuery(GET_USER, { variables: { id } });
  return data;
}
