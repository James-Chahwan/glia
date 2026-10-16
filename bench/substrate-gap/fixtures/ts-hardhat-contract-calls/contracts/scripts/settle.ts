import { ethers } from 'hardhat';
import { Contract } from 'ethers';
import { Escrow__factory } from '../typechain-types';
import EscrowArtifact from '../artifacts/contracts/Escrow.sol/Escrow.json';

export async function settle(address: string, id: string) {
  const [signer] = await ethers.getSigners();
  await Escrow__factory.connect(address, signer).release(id);
  const escrow = await ethers.getContractAt('Escrow', address);
  return escrow.deposits(id);
}

export async function settleByArtifact(address: string, id: string) {
  const [signer] = await ethers.getSigners();
  const escrow = new Contract(address, EscrowArtifact.abi, signer);
  return escrow.release(id);
}

export async function topUp(address: string, to: string) {
  const [signer] = await ethers.getSigners();
  const token = await ethers.getContractAt(
    ['function mint(address to, uint256 amount)', 'function balanceOf(address) view returns (uint256)'],
    address,
    signer,
  );
  await token.mint(to, 5);
  return token.balanceOf(to);
}
