import { BaseRepo } from './base-repo';

export class UserRepo extends BaseRepo<string> {
  fetchOne(id: string): string {
    return `user-${id}`;
  }

  protected label(): string {
    return 'users';
  }
}
