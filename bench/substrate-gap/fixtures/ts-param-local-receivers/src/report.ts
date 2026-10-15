import { ApiService } from './api.service';
import { AdminService } from './admin.service';

export function loadReport(api: ApiService, id: number): string {
  return api.fetchUser(id);
}

export class ReportBuilder {
  build(admin: AdminService, id: number): string {
    const api = new ApiService();
    const typed: ApiService = api;
    typed.purge();
    admin.purge();
    return api.fetchUser(id) + admin.fetchUser(id);
  }

  shadow(api: ApiService): void {
    const run = (api: AdminService) => api.purge();
    run(new AdminService());
  }
}
