import { APIService } from "./api.service";

// Overrides `get` with a cache lookup: a `this.get(..)` here reads the cache,
// not the HTTP client, so it is no site of APIService.get.
export class CachedViewService extends APIService {
  private cache = new Map<string, unknown>();
  private store: any;

  get(url: string) {
    return this.store.get(url);
  }

  async listCached() {
    return this.get("/api/cached/");
  }
}
