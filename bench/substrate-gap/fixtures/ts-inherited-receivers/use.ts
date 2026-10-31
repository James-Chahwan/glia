import Repo from './base';
import { Parent } from './base';

class Kid extends Parent {}

const shared = new Parent();

export function f(r: Repo, k: Kid) {
  r.size();
  Kid.make();
  return k.state + (k.ready ? 1 : 0);
}

export function g() {
  shared.count();
  return shared.state;
}

export function h(shared: Repo) {
  return shared.size();
}
