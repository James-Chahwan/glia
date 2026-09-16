// Browser WebSocket client connecting to the notifications stream at /notifications.
export function connectNotifications(): WebSocket {
  const stream = new WebSocket("wss://api.example.com/notifications");
  stream.onmessage = (ev: MessageEvent) => {
    console.log("notification", ev.data);
  };
  return stream;
}
