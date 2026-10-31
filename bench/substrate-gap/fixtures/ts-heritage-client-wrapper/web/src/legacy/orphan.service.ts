// Extends a same-named base from a package, not the app's APIService: the
// base is no in-repo class, so its verb calls mint nothing.
import { APIService } from "@vendor/sdk";

export class OrphanService extends APIService {
  async listOrphans() {
    return this.get("/api/orphans/");
  }
}
