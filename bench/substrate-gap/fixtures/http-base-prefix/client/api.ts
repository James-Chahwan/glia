// The two commonest ways a real client path fails to match the server it calls.

// (1) Interpolated base URL the repo cannot resolve. The leading segment is a
//     HOST, not a resource, so the normalised path is `/{}/users` and no server
//     route will ever be mounted at a wildcard first segment. The base is an
//     env read on purpose: the repo constant table never binds one, so this
//     call reaches the resolver's BaseFold tier. A base the table CAN resolve
//     (`${environment.apiUrl}` with `environment` declared in the repo) is
//     folded to its path by the engine first (A11.2, see angular-base-url).
export const getUsers = () => fetch(`${process.env.API_URL}/users`);

// (2) The client does not know the server mounts under /api. The old strip only
//     ever removed prefixes from the CLIENT side, so this direction never paired.
export const getOrders = () => fetch('/orders');
