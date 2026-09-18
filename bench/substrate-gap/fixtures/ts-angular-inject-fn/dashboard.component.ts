import { Component, inject } from "@angular/core";
import { ApiService } from "./api.service";

@Component({ selector: "app-dash", template: "<div></div>" })
export class DashboardComponent {
  // Angular 14+ inject() field form: DashboardComponent INJECTS ApiService.
  private api = inject(ApiService);
  // A non-inject() initialiser emits nothing.
  private tries = 3;

  load(): string[] {
    return this.api.list();
  }
}
