// An options-object builder: the path parameter (a destructured field) flows
// into a local object literal's `url` key, and that object is what the client
// call reads. No extractor knows this function.
export function makeRequest({ path }: { path: string }) {
  const opts = { url: "/api" + path, method: "GET" };
  return fetch(opts.url, opts);
}
