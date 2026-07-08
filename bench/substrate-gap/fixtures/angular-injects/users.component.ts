import { Component } from "@angular/core";
import { UserService } from "./user.service";

@Component({ selector: "app-users", template: "<ul></ul>" })
export class UsersComponent {
  // Constructor DI: UsersComponent INJECTS UserService.
  constructor(private users: UserService) {}

  refresh(): void {
    this.users.list();
  }
}
