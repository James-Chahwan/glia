export interface Account { id: string; email: string; }
export function accountLabel(a: Account): string { return a.email.toLowerCase(); }
