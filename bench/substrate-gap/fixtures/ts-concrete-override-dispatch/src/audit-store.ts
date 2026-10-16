import { BaseStore } from './base-store';

export class AuditStore extends BaseStore {
  persist(id: string): void {
    console.log(id);
  }
}
