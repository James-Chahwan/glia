import { apiBase } from "./api";

// KNOWN GAP: TS/JS IMPORTS is stubbed (engine passes a None resolver).
// This module imports apiBase from ./api; an IMPORTS edge should link them.
export function App() {
  return apiBase();
}
