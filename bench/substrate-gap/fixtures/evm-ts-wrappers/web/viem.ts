import { createPublicClient, createWalletClient, http } from 'viem';
import { hardhat } from 'viem/chains';
import { escrowAbi } from './abi';

const ESCROW_ADDRESS = '0x5FbDB2315678afecb367f032d93F642f64180aa3';
const PAYER = '0x70997970C51812dc3A010C7d01b50e0d17dc79C8';
const publicClient = createPublicClient({ chain: hardhat, transport: http() });
const walletClient = createWalletClient({ chain: hardhat, transport: http() });

async function callEscrow(functionName: string, args: readonly unknown[], value?: bigint): Promise<void> {
  await walletClient.writeContract({ account: PAYER, address: ESCROW_ADDRESS, abi: escrowAbi, functionName, args, value } as never);
}

export async function deposit(payee: `0x${string}`, amount: bigint): Promise<void> {
  await callEscrow('deposit', [payee], amount);
}

export async function release(payee: `0x${string}`, amount: bigint): Promise<void> {
  await callEscrow('release', [payee, amount]);
}

export async function readArbiter(): Promise<string> {
  return publicClient.readContract({ address: ESCROW_ADDRESS, abi: escrowAbi, functionName: 'arbiter' });
}

export function watchReleased(handler: (...args: unknown[]) => void): void {
  publicClient.watchContractEvent({ address: ESCROW_ADDRESS, abi: escrowAbi, eventName: 'Released', onLogs: handler });
}
