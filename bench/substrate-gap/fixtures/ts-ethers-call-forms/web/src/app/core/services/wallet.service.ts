import { Injectable } from '@angular/core';
import { BrowserProvider, Contract } from 'ethers';

const ESCROW_ABI = [
  'function createTrade(address buyer, uint256 amount) returns (uint256)',
  'function confirmPayment(uint256 tradeId)',
  'function releaseFunds(uint256 tradeId)',
  'function raiseDispute(uint256 tradeId)',
  'function getTrade(uint256 tradeId) view returns (tuple(uint256 id, address seller, uint256 amount))',
  'event TradeFunded(uint256 indexed tradeId, uint256 fundedAt)',
];

@Injectable({ providedIn: 'root' })
export class WalletService {
  private provider = new BrowserProvider((window as any).ethereum);
  private escrow = new Contract('0x0000000000000000000000000000000000000001', ESCROW_ABI, this.provider);

  async openTrade(buyer: string, amount: bigint): Promise<string> {
    const signer = await this.provider.getSigner();
    const escrow = new Contract('0x0000000000000000000000000000000000000001', ESCROW_ABI, signer);
    const tx = await escrow['createTrade'](buyer, amount);
    return tx.hash;
  }

  async confirm(tradeId: string): Promise<void> {
    const signer = await this.provider.getSigner();
    await this.escrow.connect(signer).confirmPayment(tradeId);
  }

  async preview(tradeId: string): Promise<boolean> {
    await this.escrow.releaseFunds.staticCall(tradeId);
    return true;
  }

  async dispute(tradeId: string): Promise<void> {
    await this.escrow['raiseDispute(uint256)'](tradeId);
  }

  async trade(tradeId: string) {
    return this.escrow.getTrade(tradeId);
  }

  async fundedSince(block: number) {
    return this.escrow.queryFilter(this.escrow.filters.TradeFunded(), block);
  }

  call(name: string, tradeId: string) {
    return this.escrow[name](tradeId);
  }
}
