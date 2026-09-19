export function connectChat(): WebSocket {
  const ws = new WebSocket("ws://chat-svc:8080/ws");
  ws.onmessage = (ev: MessageEvent) => {
    console.log("recv", ev.data);
  };
  return ws;
}
