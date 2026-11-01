import { request, gql } from 'graphql-request';

export async function listUpgrades(url: string) {
  return request(
    url,
    gql`
      {
        upgrades(first: 10) { impl }
      }
    `,
  );
}
