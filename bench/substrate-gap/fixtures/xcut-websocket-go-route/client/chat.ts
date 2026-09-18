// Browser clients of the chat server.
export function openChat(): WebSocket {
  return new WebSocket("ws://" + location.host + "/ws");
}

export function openEcho(): WebSocket {
  return new WebSocket("wss://api.example.com/echo");
}

export function openAdmin(): WebSocket {
  return new WebSocket("wss://api.example.com/admin/live");
}
