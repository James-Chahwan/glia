/** Shared loading for every repository. */
export abstract class BaseRepo<T> {
  protected load(id: string): T {
    return this.fetchOne(id);
  }

  abstract fetchOne(id: string): T;

  protected abstract label(): string;

  describe(): string {
    return this.label();
  }
}
