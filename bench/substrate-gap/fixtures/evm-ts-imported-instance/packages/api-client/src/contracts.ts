import { JsonRpcProvider, Wallet } from 'ethers';
import { Escrow__factory } from './typechain-types';
import { ESCROW_ADDRESS } from './addresses';

const provider = new JsonRpcProvider('http://localhost:8545');
const signer = Wallet.createRandom().connect(provider);
export const escrow = Escrow__factory.connect(ESCROW_ADDRESS, signer);
