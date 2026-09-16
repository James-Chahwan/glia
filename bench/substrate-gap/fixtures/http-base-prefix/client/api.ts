// The two commonest ways a real client path fails to match the server it calls.
const environment = { apiUrl: 'https://api.example.com' };

// (1) Interpolated base URL. The leading segment is a HOST, not a resource, so
//     the normalised path is `/{}/users` and no server route will ever be
//     mounted at a wildcard first segment.
export const getUsers = () => fetch(`${environment.apiUrl}/users`);

// (2) The client does not know the server mounts under /api. The old strip only
//     ever removed prefixes from the CLIENT side, so this direction never paired.
export const getOrders = () => fetch('/orders');
