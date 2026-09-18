export function connectChat(): WebSocket {
  return new WebSocket("ws://" + location.host + "/ws/chat");
}

export function connectAdmin(base: string): WebSocket {
  return new WebSocket(`${base}/ws/admin`);
}

export function connectUnknown(url: string): WebSocket {
  return new WebSocket(url);
}
