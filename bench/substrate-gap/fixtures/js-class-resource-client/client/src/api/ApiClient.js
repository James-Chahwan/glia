/* global axios */

const DEFAULT_API_VERSION = 'v1';

// The resource client every API module extends: the URL is built from the
// resource name and options each subclass hands to super().
class ApiClient {
  constructor(resource, options = {}) {
    this.apiVersion = `/api/${options.apiVersion || DEFAULT_API_VERSION}`;
    this.options = options;
    this.resource = resource;
  }

  get url() {
    return `${this.baseUrl()}/${this.resource}`;
  }

  get accountIdFromRoute() {
    const parts = window.location.pathname.split('/');
    return parts[3] || '';
  }

  baseUrl() {
    let url = this.apiVersion;

    if (this.options.enterprise) {
      url = `/enterprise${url}`;
    }

    if (this.options.accountScoped && this.accountIdFromRoute) {
      url = `${url}/accounts/${this.accountIdFromRoute}`;
    }

    return url;
  }

  show(id) {
    return axios.get(`${this.url}/${id}`);
  }
}

export default ApiClient;
