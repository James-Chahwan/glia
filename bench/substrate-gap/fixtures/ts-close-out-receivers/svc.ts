import { Api } from './api';

export class Svc {
  load(): Promise<Item> {
    return Promise.resolve(new Item());
  }
}

export class Item {
  touch() {}
}

export function url() {
  return Api.baseUrl;
}

export async function useIt(s: Svc) {
  const it = await s.load();
  it.touch();
  function inner() {
    Api.ping();
  }
  inner();
}
