// Browser WebSocket client connecting to the Go chat server at /ws.
export function connectChat(): WebSocket {
  const ws = new WebSocket("ws://localhost:8080/ws");
  ws.onopen = () => {
    ws.send(JSON.stringify({ type: "hello" }));
  };
  ws.onmessage = (ev: MessageEvent) => {
    console.log("recv", ev.data);
  };
  return ws;
}
