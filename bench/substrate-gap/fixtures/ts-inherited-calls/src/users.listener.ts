import { Injectable } from '@nestjs/common';
import { OnEvent } from '@nestjs/event-emitter';
import { UserRepo } from './user-repo';

@Injectable()
export class UsersListener {
  constructor(private readonly repo: UserRepo) {}

  @OnEvent('user.requested')
  onRequested(id: string): string {
    return this.repo.find(id);
  }
}
