import { Injectable } from "@angular/core";
import { HttpClient } from "@angular/common/http";

@Injectable({ providedIn: "root" })
export class UsersApi {
  constructor(private http: HttpClient) {}

  getUsers() {
    return this.http.get("/users");
  }
}
