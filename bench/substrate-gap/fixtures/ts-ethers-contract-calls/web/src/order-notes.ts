import { Contract, type Signer } from 'ethers';

const ESCROW_ABI = ['function release(bytes32 orderId)', 'function deposit(bytes32 orderId) payable'];

export async function releaseNow(address: string, signer: Signer, id: string) {
  const escrow = new Contract(address, ESCROW_ABI, signer);
  return escrow.release(id);
}

export function noteRelease(escrow: { release(id: string): string }, id: string) {
  return escrow.release(id);
}
