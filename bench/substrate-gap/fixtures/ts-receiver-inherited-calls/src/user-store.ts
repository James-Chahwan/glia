import { BaseStore } from './base-store';

export class UserStore extends BaseStore {
  find(id: string): string {
    return this.load(id);
  }
}
