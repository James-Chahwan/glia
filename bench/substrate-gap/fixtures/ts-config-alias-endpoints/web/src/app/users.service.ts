import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { ADMIN_BASE } from './admin-base.util';

@Injectable({ providedIn: 'root' })
export class UsersService {
  constructor(private http: HttpClient) {}
  list() {
    return this.http.get(`${ADMIN_BASE}/users`);
  }
}
