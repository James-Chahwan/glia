// WebSocket client: web -> api over /ws (WS_CONNECTS).
export function connectChat(): WebSocket {
  const ws = new WebSocket("ws://localhost:8080/ws");
  ws.onmessage = (ev: MessageEvent) => {
    console.log("recv", ev.data);
  };
  return ws;
}
