import { deployContract } from 'ethereum-waffle';
import type { Wallet } from 'ethers';
import Lock from './build/Lock.json';

export async function lockUp(wallet: Wallet) {
  const lock = await deployContract(wallet, Lock);
  await lock.unlock();
}
