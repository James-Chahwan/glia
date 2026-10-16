const Web3 = require('web3');

const ESCROW_JSON = [
  { type: 'function', name: 'release', inputs: [{ name: 'orderId', type: 'bytes32' }] },
  { type: 'function', name: 'deposit', inputs: [{ name: 'orderId', type: 'bytes32' }], stateMutability: 'payable' },
  { type: 'event', name: 'Released', inputs: [{ name: 'orderId', type: 'bytes32', indexed: true }, { name: 'seller', type: 'address' }] },
];

async function releaseLegacy(web3, address, id, from) {
  const escrow = new web3.eth.Contract(ESCROW_JSON, address);
  escrow.events.Released({}, () => {});
  return escrow.methods.release(id).send({ from });
}

module.exports = { releaseLegacy, Web3 };
