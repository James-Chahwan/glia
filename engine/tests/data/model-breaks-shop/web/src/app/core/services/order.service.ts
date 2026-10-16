import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { Observable } from 'rxjs';
import { environment } from '../../../environments/environment';
import { NewOrder, Order } from '../models/order';

@Injectable({ providedIn: 'root' })
export class OrderService {
  private readonly base = `${environment.apiUrl}/orders`;

  constructor(private http: HttpClient) {}

  get(id: string): Observable<Order> {
    return this.http.get<Order>(`${this.base}/${id}`);
  }

  place(order: NewOrder): Observable<Order> {
    return this.http.post<Order>(this.base, order);
  }

  cancel(id: string): Observable<Order> {
    return this.http.patch<Order>(`${this.base}/${id}/status`, { status: 'cancelled' });
  }
}
