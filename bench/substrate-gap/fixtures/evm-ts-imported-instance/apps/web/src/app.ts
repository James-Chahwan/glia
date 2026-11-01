import { escrow, fmt } from '@sibyl/api-client';

const MEMO = `0x${'00'.repeat(32)}` as const;

export async function checkout(payee: `0x${string}`, amount: bigint): Promise<string> {
  await escrow.deposit(payee, { value: amount });
  await escrow.release(payee, amount, MEMO);
  void escrow.on(escrow.filters.Released(), () => undefined);
  console.log(fmt.amount(amount));
  return escrow.arbiter();
}
