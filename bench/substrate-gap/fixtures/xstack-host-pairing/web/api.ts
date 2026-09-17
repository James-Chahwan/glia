import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { environment } from './environment';

@Injectable({ providedIn: 'root' })
export class UsersApi {
  constructor(private http: HttpClient) {}

  getUsers() {
    return this.http.get(`${environment.apiUrl}/users`);
  }

  getUser(id: string) {
    return this.http.get(`${environment.apiUrl}/users/${id}`);
  }
}
