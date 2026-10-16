import hre from 'hardhat';

export async function releaseOnChain(id: `0x${string}`) {
  const escrow = await hre.viem.deployContract('Escrow');
  await escrow.write.deposit([id], { value: 1n });
  return escrow.write.release([id]);
}
