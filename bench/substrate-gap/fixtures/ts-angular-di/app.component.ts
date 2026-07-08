import { Component } from "@angular/core";
import { ApiService } from "./api.service";

@Component({ selector: "app-root", template: "<div></div>" })
export class AppComponent {
  // Constructor DI: AppComponent INJECTS ApiService.
  constructor(private api: ApiService) {}

  load(): void {
    this.api.getUsers();
  }
}
