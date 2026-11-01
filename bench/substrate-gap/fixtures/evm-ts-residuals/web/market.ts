import Contract from 'web3-eth-contract';

const MARKET_ABI = [
  { type: 'function', name: 'list', inputs: [{ name: 'item', type: 'uint256' }], outputs: [], stateMutability: 'nonpayable' },
];

export function listItem(addr: string, from: string) {
  const market = new Contract(MARKET_ABI, addr);
  return market.methods.list(1).send({ from });
}
