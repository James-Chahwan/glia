export class BaseStore {
  constructor() {}

  load(id: string): string {
    return this.key(id);
  }

  key(id: string): string {
    return `k-${id}`;
  }

  save(id: string): void {
    this.persist(id);
  }

  persist(id: string): void {}
}
