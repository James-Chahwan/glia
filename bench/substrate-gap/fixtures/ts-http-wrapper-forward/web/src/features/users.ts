import { api } from "../lib/http";
import { getJson, readResource, sendJson } from "../lib/verbs";

export function listUsers() {
  return getJson("/users");
}

export function createUser(user: unknown) {
  return sendJson("POST", "/users", user);
}

export function deleteUser(id: string) {
  return api(`/users/${id}`, { method: "DELETE" });
}

export function listTeams() {
  return readResource("/teams");
}

export function lookup(key: string) {
  // getJson("/legacy") was the old listing.
  return getJson(key);
}

export function cacheKey() {
  return getJson("cache key");
}
