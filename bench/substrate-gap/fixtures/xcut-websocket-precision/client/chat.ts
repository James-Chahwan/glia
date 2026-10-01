// Browser WebSocket client connecting to the chat stream the server upgrades at /chat.
export function connectChat(): WebSocket {
  const socket = new WebSocket("wss://api.example.com/chat");
  socket.onmessage = (ev: MessageEvent) => {
    console.log("chat", ev.data);
  };
  return socket;
}
