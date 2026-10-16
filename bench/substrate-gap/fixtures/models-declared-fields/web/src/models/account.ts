export interface Audited {
  createdAt: string;
  updatedAt?: string;
}

export interface Account extends Audited {
  readonly id: string;
  'display-name': string;
  email?: string;
  roles: string[];
  owner: Person;
  isActive(): boolean;
}

export interface Settings {
  theme: 'light' | 'dark';
  [flag: string]: unknown;
}

export class Person {
  name = '';
  age?: number;
  constructor(public nickname: string) {}
}

export class AccountStore {
  private cache = new Map<string, Account>();

  get(id: string): Account | undefined {
    return this.cache.get(id);
  }
}
