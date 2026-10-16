import { BrowserProvider, Contract } from 'ethers';

const ESCROW_ABI = [
  'function deposit(bytes32 orderId) payable',
  'function release(bytes32 orderId)',
  'function deposits(bytes32) view returns (uint256)',
  'event Released(bytes32 indexed orderId, address seller)',
];

const ERC20_ABI = [
  'function balanceOf(address) view returns (uint256)',
  'function decimals() view returns (uint8)',
];

export class EscrowClient {
  constructor(private provider: BrowserProvider, private address: string, private token: string) {}

  async pay(orderId: string, value: bigint) {
    const signer = await this.provider.getSigner();
    const escrow = new Contract(this.address, ESCROW_ABI, signer);
    const tx = await escrow.deposit(orderId, { value });
    console.log(await escrow.getAddress());
    return tx.wait();
  }

  async settle(orderId: string) {
    const signer = await this.provider.getSigner();
    const escrow = new Contract(this.address, ESCROW_ABI, signer);
    // escrow.withdraw(orderId) would revert: Escrow has no withdraw
    return escrow.release(orderId);
  }

  held(orderId: string) {
    return new Contract(this.address, ESCROW_ABI, this.provider).deposits(orderId);
  }

  watch(onRelease: (id: string) => void) {
    const escrow = new Contract(this.address, ESCROW_ABI, this.provider);
    escrow.on('Released', (orderId: string) => onRelease(orderId));
  }

  async tokenDecimals() {
    const token = new Contract(this.token, ERC20_ABI, this.provider);
    return token.decimals();
  }

  async stray(orderId: string) {
    const escrow = new Contract(this.address, ESCROW_ABI, this.provider);
    return escrow.withdraw(orderId);
  }
}
