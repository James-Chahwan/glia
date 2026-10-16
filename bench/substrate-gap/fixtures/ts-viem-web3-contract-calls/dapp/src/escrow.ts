import { getContract, parseAbi, type PublicClient, type WalletClient } from 'viem';
import { useReadContract } from 'wagmi';

export const escrowAbi = parseAbi([
  'function deposit(bytes32 orderId) payable',
  'function release(bytes32 orderId)',
  'function deposits(bytes32) view returns (uint256)',
  'event Released(bytes32 indexed orderId, address seller)',
]);

export function releaseOrder(wallet: WalletClient, address: `0x${string}`, id: `0x${string}`) {
  return wallet.writeContract({ address, abi: escrowAbi, functionName: 'release', args: [id] });
}

export function watchReleases(client: PublicClient, address: `0x${string}`, onLogs: (logs: unknown[]) => void) {
  return client.watchContractEvent({ address, abi: escrowAbi, eventName: 'Released', onLogs });
}

export function useHeld(address: `0x${string}`, id: `0x${string}`) {
  return useReadContract({ address, abi: escrowAbi, functionName: 'deposits', args: [id] });
}

export async function heldViaInstance(client: PublicClient, address: `0x${string}`, id: `0x${string}`) {
  const escrow = getContract({ address, abi: escrowAbi, client });
  return escrow.read.deposits([id]);
}
