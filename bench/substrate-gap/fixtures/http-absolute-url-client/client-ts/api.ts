// A3.3: an SPA client calling its API by ABSOLUTE URL with a query string —
// the default shape once a base URL is inlined. The ENDPOINT must be keyed on
// the request path `/users`, not the whole URL, or it can never pair.
export const listUsers = () => fetch('https://api.example.com/users?active=1');
