import { Client } from '@stomp/stompjs';
import SockJS from 'sockjs-client';

type Listener = (body: string) => void;

export function connectLive(url: string, onClosed: () => void): Client {
  const client = new Client({
    webSocketFactory: () => new SockJS(url),
    onWebSocketClose: () => fireDisconnected(onClosed),
  });
  client.activate();
  return client;
}

function fireDisconnected(onClosed: () => void): void {
  onClosed();
}

export function watchProject(client: Client, id: number, cb: Listener): void {
  client.subscribe(`/projects/${id}/changes`, (message) => cb(message.body));
}

export function watchNotifications(client: Client, cb: Listener): void {
  client.subscribe('/user/notifications', (message) => cb(message.body));
}
