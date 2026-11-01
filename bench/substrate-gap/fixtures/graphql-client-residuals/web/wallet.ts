import { graphqlRequest } from './transport';

const LINK_WALLET =
  "mutation LinkWallet($address: String!) {" +
  " linkWallet(address: $address) { address } }";

export async function linkWallet(address: string) {
  return graphqlRequest(LINK_WALLET, { address });
}
