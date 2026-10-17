import { Router } from '@angular/router';
import { DraftStore } from './draft.store';

export function makeDraft(): DraftStore {
  return new DraftStore();
}

export function makeAny() {
  return new DraftStore();
}

export function routerOf(router: Router): Router {
  return router;
}
