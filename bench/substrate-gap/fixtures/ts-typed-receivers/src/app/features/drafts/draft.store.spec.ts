import { DraftStore } from './draft.store';
import { makeAny, makeDraft, routerOf } from './testing';

function makeStore(): DraftStore {
  return new DraftStore();
}

function withFactory(makeStore: () => any): void {
  const s = makeStore();
  s.drop();
}

describe('DraftStore', () => {
  it('resets', () => {
    const store = makeStore();
    store.reset();
  });

  it('patches through an imported factory', () => {
    const other = makeDraft();
    other.patch('x');
  });

  it('reads nothing off an unannotated factory', () => {
    const loose = makeAny();
    loose.drop();
    withFactory(makeStore);
  });

  it('reads no repo getter through a third-party return type', () => {
    const nav = routerOf({} as never);
    return nav.url;
  });
});
