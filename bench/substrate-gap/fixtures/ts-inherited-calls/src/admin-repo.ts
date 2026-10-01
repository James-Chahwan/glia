import { Injectable } from '@nestjs/common';
import { UserRepo } from './user-repo';

@Injectable()
export class AdminRepo extends UserRepo {
  findAdmin(id: string): string {
    return this.load(id) + this.find(id) + this.describe();
  }

  audit(): void {
    this.missing();
  }
}
