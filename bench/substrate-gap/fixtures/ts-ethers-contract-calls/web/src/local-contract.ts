class Contract {
  constructor(private a: string, private b: string[], private c: unknown) {}
  deposit(x: string) { return x; }
}

export function localDeposit(id: string) {
  const escrow = new Contract('0x0', [], null);
  return escrow.deposit(id);
}
