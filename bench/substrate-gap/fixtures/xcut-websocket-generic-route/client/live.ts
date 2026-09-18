// Browser client of the live feed.
export function openLive(): WebSocket {
  return new WebSocket("wss://api.example.com/live");
}
