// A typed client against a method-agnostic server route. Both verbs target the
// same declaration on the server; neither can pair unless the route index
// treats "ANY" as a wildcard.
export const listPosts = () => fetch('/posts');

export const addPost = (b: unknown) =>
  fetch('/posts', { method: 'POST', body: JSON.stringify(b) });
