export default class Repo {
  size(): number {
    return 1;
  }
}

function initial(): number {
  return 0;
}

export class Parent {
  state = initial();
  static make(): Parent {
    return new Parent();
  }
  get ready(): boolean {
    return true;
  }
  count(): number {
    return 2;
  }
}
