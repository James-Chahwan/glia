import { decodeEventLog, encodeFunctionData, writeContract } from 'viem';
import { escrowAbi, vaultAbi } from './abi';

const ESCROW_ADDRESS = '0x0000000000000000000000000000000000000001';
const escrowContract = { address: ESCROW_ADDRESS, abi: escrowAbi } as const;

export function sweepData(to: string) {
  return encodeFunctionData({ abi: vaultAbi, functionName: 'sweep', args: [to] });
}

export function released(log: { data: string; topics: string[] }) {
  return decodeEventLog({ abi: escrowAbi, eventName: 'Released', data: log.data, topics: log.topics });
}

export async function releaseIt(config: unknown, id: bigint) {
  return writeContract(config, { ...escrowContract, functionName: 'release', args: [id] });
}
