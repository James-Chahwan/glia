import { ethers, upgrades } from 'hardhat';
import { Contract, ContractFactory, type Signer } from 'ethers';
import { escrowAbi } from './abi';

const TOKEN_ABI = ['function mint(uint256 amount)', 'function burn(uint256 amount)'];

export async function settle(addr: string, p: Signer) {
  const e = await ethers.getContract('Escrow');
  await e.release(1);
  return new Contract(addr, escrowAbi, p).refund(1);
}

export async function launch(bytecode: string, signer: Signer, other: Signer) {
  const f = new ContractFactory(TOKEN_ABI, bytecode, signer);
  const t = await f.deploy();
  await t.mint(1);
  const f2 = f.connect(other);
  const t2 = await f2.deploy();
  await t2.burn(1);
}

export async function govern() {
  const g = await (await ethers.getContractFactory('Gov')).deploy();
  await g.propose(1);
  const pool = await upgrades.deployProxy(await ethers.getContractFactory('Pool'), []);
  await pool.stake(1);
}
