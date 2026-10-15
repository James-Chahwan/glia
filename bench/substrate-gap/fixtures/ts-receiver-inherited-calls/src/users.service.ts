import { Injectable } from '@nestjs/common';
import { UserStore } from './user-store';

const repo = makeRepo();

function makeRepo(): unknown {
  return {};
}

@Injectable()
export class UsersService {
  constructor(private readonly store: UserStore, private readonly repo: UserStore) {}

  onRequested(id: string): string {
    this.store.save(id);
    return this.store.load(id);
  }

  sync(store: UserStore, id: string): string {
    return store.find(id) + store.load(id);
  }

  rebuild(id: string): string {
    const fresh = new UserStore();
    fresh.save(id);
    return new UserStore().load(id);
  }

  stale(id: string): string {
    return repo.load(id);
  }
}
