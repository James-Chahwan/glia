export function connectChat(onMessage: (text: string) => void): WebSocket {
  const socket = new WebSocket("ws://api.example.com/ws/chat");
  socket.onmessage = (event) => onMessage(String(event.data));
  return socket;
}
