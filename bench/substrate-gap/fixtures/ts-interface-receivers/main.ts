import { Store } from './store';

export function run(s: Store) {
  s.load('x');
  s.save('y');
}

export class Holder {
  #store: Store;
  constructor(s: Store) {
    this.#store = s;
  }
  go() {
    this.#store.load('z');
  }
}
