import { BrowserProvider, Contract, parseEther } from 'ethers';

const ESCROW_ABI = [
  'function deposit(bytes32 orderId) payable',
  'function release(bytes32 orderId)',
  'event Released(bytes32 indexed orderId, address seller)',
];

export async function payIntoEscrow(address: string, orderId: string, amountEth: string): Promise<void> {
  const provider = new BrowserProvider((window as any).ethereum);
  const signer = await provider.getSigner();
  const escrow = new Contract(address, ESCROW_ABI, signer);
  const tx = await escrow.deposit(orderId, { value: parseEther(amountEth) });
  await tx.wait();
}
