export class Store {
  #count = 0;

  notify() {}

  increment = () => {
    this.#count += 1;
    this.notify();
  };

  static create = () => new Store();
}
