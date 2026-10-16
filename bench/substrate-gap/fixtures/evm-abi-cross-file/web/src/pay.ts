import { Contract, type Signer } from 'ethers';
import { SHOP_ABI } from './abi';

export async function payAndRelease(address: string, signer: Signer, id: string) {
  const shop = new Contract(address, SHOP_ABI, signer);
  await shop.deposit(id, { value: 1n });
  return shop.release(id);
}
