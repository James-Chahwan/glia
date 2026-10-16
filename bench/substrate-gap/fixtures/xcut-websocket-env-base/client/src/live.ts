import { environment } from './environment';

export function openFeed(): WebSocket {
  return new WebSocket(environment.feedUrl);
}

export function openPrices(): WebSocket {
  return new WebSocket(`${environment.wsBase}/prices`);
}
