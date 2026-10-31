import { Store } from './store';

export class PgStore implements Store {
  load(id: string) {
    return id;
  }
  save(v: string) {}
}
