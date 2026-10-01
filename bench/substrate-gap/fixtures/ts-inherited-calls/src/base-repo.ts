export abstract class BaseRepo {
  protected load(id: string): string {
    return this.fetchOne(id);
  }

  abstract fetchOne(id: string): string;

  describe(): string {
    return 'repo';
  }
}
