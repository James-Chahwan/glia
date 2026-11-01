import { JsonRpcProvider, Wallet } from 'ethers';
import { Escrow__factory } from './typechain-types';

const ESCROW_ADDRESS = '0x5FbDB2315678afecb367f032d93F642f64180aa3';
const provider = new JsonRpcProvider('http://127.0.0.1:8545');
const signer = new Wallet(process.env.PAYER_KEY ?? '', provider);
const escrow = Escrow__factory.connect(ESCROW_ADDRESS, signer);

async function callEscrow(method: string, args: unknown[], value?: bigint): Promise<unknown> {
  const fn = escrow.getFunction(method);
  return value === undefined ? fn(...args) : fn(...args, { value });
}

export async function deposit(payee: string, amount: bigint): Promise<void> {
  await callEscrow('deposit', [payee], amount);
}

export async function release(payee: string, amount: bigint): Promise<void> {
  await callEscrow('release', [payee, amount]);
}

export async function readArbiter(): Promise<string> {
  return escrow.arbiter();
}

export function watchReleased(handler: (...args: unknown[]) => void): void {
  void escrow.on(escrow.filters.Released(), handler);
}
