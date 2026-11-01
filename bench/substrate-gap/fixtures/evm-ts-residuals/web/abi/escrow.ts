export const escrowAbi = [
  'function release(uint256 id)',
  'function refund(uint256 id)',
  'event Released(uint256 indexed id)',
] as const;
