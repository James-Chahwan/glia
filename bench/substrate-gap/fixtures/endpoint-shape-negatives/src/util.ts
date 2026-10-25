// None of these is an HTTP call, whatever the shape rule reads.

export function log(path: string, ...rest: unknown[]) {
  console.log(path, rest);
}

export function noise() {
  // a path-shaped string, but no method signal
  log('/tmp/x');
  log('/var/log/app', { level: 'info' });
  // a method signal, but the string is no path
  request('users list', { method: 'POST' });
  // data: / blob: / mailto: URLs are never endpoints
  fetch('data:image/png;base64,iVBORw0KGgo=');
  fetchBlob('blob:https://example.com/550e8400-e29b-41d4-a716-446655440000');
  request('mailto:ops@example.com', { method: 'GET' });
}

function request(target: string, opts: { method: string }) {
  return [target, opts];
}

function fetchBlob(url: string) {
  return url;
}
