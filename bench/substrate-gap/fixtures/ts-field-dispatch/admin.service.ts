import { Injectable } from "@angular/core";

@Injectable({ providedIn: "root" })
export class AdminService {
  purge(): void {}

  fetchUser(id: number): string {
    return "admin" + id;
  }
}
