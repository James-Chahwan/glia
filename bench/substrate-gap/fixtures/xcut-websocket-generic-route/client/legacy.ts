// A client still pointed at the old /ws path, which no route serves.
export function openLegacy(): WebSocket {
  return new WebSocket("wss://api.example.com/ws");
}
