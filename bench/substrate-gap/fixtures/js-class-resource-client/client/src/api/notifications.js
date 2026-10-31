/* global axios */
import ApiClient from './ApiClient';

class NotificationsAPI extends ApiClient {
  constructor() {
    super('notifications', { accountScoped: true });
  }

  list() {
    return axios.get(this.url);
  }

  markUnread(id) {
    return axios.post(`${this.url}/${id}/unread`);
  }
}

export default new NotificationsAPI();
