import { APIService } from "./api.service";

export class ViewService extends APIService {
  constructor(baseURL?: string) {
    super(baseURL || "");
  }

  async fetchView(id: string) {
    return this.get(`/api/views/${id}/`).then((response) => response?.data);
  }

  async createView(data: object) {
    return this.post("/api/views/", data).then((response) => response?.data);
  }

  async describe(key: string) {
    // this.get("/api/legacy/") was the old view listing.
    return this.get(key);
  }
}
