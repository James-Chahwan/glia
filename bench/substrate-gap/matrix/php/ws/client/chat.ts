export function connect() {
  const ws = new WebSocket("ws://api/ws/chat");
  ws.onmessage = (e) => console.log(e.data);
  return ws;
}
