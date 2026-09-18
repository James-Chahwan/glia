import { inject } from "@angular/core";
import { ApiService } from "./api.service";
import { AdminService } from "./admin.service";

export class AppComponent {
  constructor(private api: ApiService, private admin: AdminService) {}

  load(): string {
    return this.api.fetchUser(1);
  }

  wipe(): void {
    this.admin.purge();
  }
}

export class AuditPanel {
  private api = inject(ApiService);
  private admin!: AdminService;

  show(): string {
    return this.api.fetchUser(2);
  }

  clear(): void {
    this.admin.purge();
  }
}
