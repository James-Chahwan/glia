// A Map-like store: `get(id)` / `delete(id)` read like HTTP verbs, but the
// receiver is no HTTP client, so neither method is an HTTP wrapper.
export class Cache {
  private m = new Map<string, number>();
  private entries: any;

  read(id: string) {
    return this.m.get(id);
  }

  lookup(key: string) {
    return this.entries.get(key);
  }

  drop(key: string) {
    return this.entries.delete(key);
  }
}
