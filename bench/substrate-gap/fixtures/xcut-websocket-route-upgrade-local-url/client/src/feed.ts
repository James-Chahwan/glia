import { environment } from './environment';
import { UrlBuilder } from './urls';

export class Feed {
  constructor(private readonly urls: UrlBuilder) {}

  connect(): WebSocket {
    const url = `${environment.liveUrl}/live/orders`;
    return new WebSocket(url);
  }

  chat(): WebSocket {
    const url = this.urls.buildWsUrl('api/chat/ws');
    return new WebSocket(url);
  }

  admin(): WebSocket {
    let url = '/admin/live';
    url = url + '?debug=1';
    return new WebSocket(url);
  }
}
