export const escrowAbi = [
  { type: 'function', name: 'deposit', stateMutability: 'payable', inputs: [{ name: 'payee', type: 'address' }], outputs: [] },
  { type: 'function', name: 'release', stateMutability: 'nonpayable', inputs: [{ name: 'payee', type: 'address' }, { name: 'amount', type: 'uint256' }], outputs: [] },
  { type: 'function', name: 'arbiter', stateMutability: 'view', inputs: [], outputs: [{ name: '', type: 'address' }] },
  { type: 'event', name: 'Released', inputs: [{ name: 'payee', type: 'address', indexed: true }, { name: 'amount', type: 'uint256', indexed: false }] },
] as const;
