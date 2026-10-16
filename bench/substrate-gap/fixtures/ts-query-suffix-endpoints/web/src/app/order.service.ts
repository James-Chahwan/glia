import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { environment } from '../environments/environment';

@Injectable({ providedIn: 'root' })
export class OrderService {
  private readonly base = `${environment.apiUrl}/orders`;

  constructor(private http: HttpClient) {}

  list(status?: string) {
    const query = status ? `?status=${status}` : '';
    return this.http.get(`${this.base}${query}`);
  }

  get(id: string) {
    return this.http.get(`${this.base}/${id}`);
  }
}
