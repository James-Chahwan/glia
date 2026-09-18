export function openEcho(): WebSocket {
  return new WebSocket("wss://api.example.com/echo");
}

export function openNotifications(): WebSocket {
  return new WebSocket(`wss://api.example.com/notifications`);
}
