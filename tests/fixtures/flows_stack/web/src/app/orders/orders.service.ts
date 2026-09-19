import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';

@Injectable({ providedIn: 'root' })
export class OrdersService {
  constructor(private http: HttpClient) {}

  list() {
    return this.http.get('/api/orders');
  }
}
