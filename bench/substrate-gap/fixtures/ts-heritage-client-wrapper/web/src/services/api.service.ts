// The app's HTTP base class: every service extends it and calls its verb
// members on `this`. No extractor knows these members.
import axios from "axios";
import type { AxiosInstance } from "axios";

export abstract class APIService {
  protected baseURL: string;
  private axiosInstance: AxiosInstance;

  constructor(baseURL: string) {
    this.baseURL = baseURL;
    this.axiosInstance = axios.create({ baseURL, withCredentials: true });
  }

  get(url: string, params = {}) {
    return this.axiosInstance.get(url, { ...params });
  }

  post(url: string, data = {}) {
    return this.axiosInstance.post(url, data);
  }
}
