import { Injectable } from '@nestjs/common';
import { BaseRepo } from './base-repo';

@Injectable()
export class UserRepo extends BaseRepo {
  fetchOne(id: string): string {
    return `user-${id}`;
  }

  describe(): string {
    return `users: ${super.describe()}`;
  }

  find(id: string): string {
    return this.load(id);
  }
}
