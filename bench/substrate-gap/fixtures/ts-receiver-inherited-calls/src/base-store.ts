export class BaseStore {
  load(id: string): string {
    return `k-${id}`;
  }

  save(id: string): void {}
}
