/* global axios */
import ApiClient from './ApiClient';

// Control: the resource is a runtime value, never a literal handed to
// super(), so no call here folds.
export class DynamicAPI extends ApiClient {
  constructor(name) {
    super(name, { accountScoped: true });
  }

  search(query) {
    return axios.get(`${this.url}/search`, { params: { query } });
  }
}
