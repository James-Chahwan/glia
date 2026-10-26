import { makeRequest } from "../lib/rest";
import { Cache } from "../lib/cache";

export function loadItems() {
  return makeRequest({ path: "/items" });
}

export function cached(c: Cache) {
  c.lookup("/items");
  c.drop("/items");
  return c.read("/items");
}
