import { Order } from './models';

export class Repo {
  async find<T>(id: number): Promise<T> {
    return { id } as T;
  }
}

export class Store {
  constructor(private repo: Repo) {}

  async load(id: number): Promise<Order> {
    return await this.repo.find<Order>(id);
  }
}
