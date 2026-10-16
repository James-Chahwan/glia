import { Account, accountLabel } from '../models';
export class AccountService { label(a: Account): string { return accountLabel(a); } }
