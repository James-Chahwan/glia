import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';

@Injectable({ providedIn: 'root' })
export class AuthService {
  constructor(private http: HttpClient) {}
  reset(token: string, password: string) {
    return this.http.post(`/api/auth/password/reset/${token}`, { password });
  }
  boom() {
    return this.http.get('/api/boom');
  }
  abTest() {
    return this.http.get('/api/ab_test');
  }
  orders() {
    return this.http.get('/api/orders');
  }
}
