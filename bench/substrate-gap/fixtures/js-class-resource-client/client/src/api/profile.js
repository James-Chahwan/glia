/* global axios */
import ApiClient from './ApiClient';

class ProfileAPI extends ApiClient {
  constructor() {
    super('profile');
  }

  current() {
    return axios.get(this.url);
  }

  save(data) {
    return axios.put(this.url, data);
  }
}

export default new ProfileAPI();
