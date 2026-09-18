// Browser clients of the room endpoint.
export function joinRoom(): WebSocket {
  return new WebSocket("wss://chat.example.com/chat/lobby");
}

export function openFeed(): WebSocket {
  return new WebSocket("wss://chat.example.com/chat/lobby/feed");
}
