import { Injectable } from '@angular/core';
import { BrowserProvider, Contract } from 'ethers';

const WALLET_ESCROW_ABI = [
  'function confirm(bytes32 orderId)',
  'function release(bytes32 orderId)',
];

const ERC20_ABI = [
  'function balanceOf(address) view returns (uint256)',
  'function approve(address spender, uint256 amount) returns (bool)',
];

@Injectable({ providedIn: 'root' })
export class WalletService {
  private provider = new BrowserProvider((window as any).ethereum);

  async confirmOrder(address: string, orderId: string): Promise<string> {
    const signer = await this.provider.getSigner();
    const escrow = new Contract(address, WALLET_ESCROW_ABI, signer);
    const tx = await escrow['confirm'](orderId);
    return tx.hash;
  }

  async approveSpend(token: string, spender: string, amount: bigint): Promise<string> {
    const signer = await this.provider.getSigner();
    const erc20 = new Contract(token, ERC20_ABI, signer);
    const tx = await erc20['approve'](spender, amount);
    return tx.hash;
  }
}
