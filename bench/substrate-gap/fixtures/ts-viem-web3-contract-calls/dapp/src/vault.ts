import type { PublicClient } from 'viem';

export const vaultAbi = [
  { type: 'function', name: 'withdraw', stateMutability: 'nonpayable', inputs: [{ name: 'amount', type: 'uint256' }], outputs: [] },
  { type: 'function', name: 'release', stateMutability: 'nonpayable', inputs: [{ name: 'id', type: 'bytes32' }], outputs: [] },
] as const;

export function withdrawFromVault(client: PublicClient, address: `0x${string}`, amount: bigint) {
  return client.simulateContract({ address, abi: vaultAbi, functionName: 'withdraw', args: [amount] });
}
