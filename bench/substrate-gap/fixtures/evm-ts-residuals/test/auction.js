const Auction = artifacts.require('Auction');

const AUCTION_ABI = [
  { type: 'function', name: 'bid', inputs: [{ name: 'amount', type: 'uint256' }], outputs: [], stateMutability: 'payable' },
];

contract('Auction', (accounts) => {
  it('bids', async () => {
    const deployed = await Auction.deployed();
    const auction = new web3.eth.Contract(AUCTION_ABI, deployed.address);
    await auction.methods.bid(1).send({ from: accounts[0], value: 1 });
  });
});
