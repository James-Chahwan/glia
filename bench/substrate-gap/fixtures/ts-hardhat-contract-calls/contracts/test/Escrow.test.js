const { expect } = require('chai');
const { ethers } = require('hardhat');

describe('Escrow', function () {
  let escrow;
  let token;

  beforeEach(async function () {
    const Escrow = await ethers.getContractFactory('Escrow');
    escrow = await Escrow.deploy();
    token = await ethers.deployContract('Token');
    await token.waitForDeployment();
  });

  it('holds a deposit', async function () {
    const [buyer] = await ethers.getSigners();
    const id = ethers.id('order-1');
    await escrow.connect(buyer).deposit(id, { value: 1 });
    await token.mint(buyer.address, 5);
    await token.approve(await escrow.getAddress(), 5);
    expect(await escrow.deposits(id)).to.equal(1);
    await expect(escrow.release(id)).to.emit(escrow, 'Released');
  });
});
