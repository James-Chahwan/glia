import { BaseStore } from './base-store';

export class UserStore extends BaseStore {
  constructor() {
    super();
  }

  key(id: string): string {
    return `user-${super.key(id)}`;
  }
}
