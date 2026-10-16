import { HttpClient } from '@angular/common/http';
import { Injectable } from '@angular/core';
import { environment } from '../../environments/environment';
import { Invoice, InvoiceStatus, NewInvoice } from '../models/invoice';

@Injectable({ providedIn: 'root' })
export class InvoiceService {
  private readonly base = `${environment.apiUrl}/invoices`;

  constructor(private http: HttpClient) {}

  get(id: string) {
    return this.http.get<Invoice>(`${this.base}/${id}`);
  }

  create(inv: NewInvoice) {
    return this.http.post<Invoice>(this.base, inv);
  }

  setStatus(id: string, status: InvoiceStatus) {
    return this.http.patch<Invoice>(`${this.base}/${id}/status`, { status });
  }

  page(n: number) {
    return this.http.get<{ invoices: Invoice[]; page: number }>(`${environment.apiUrl}/invoice-pages/${n}`);
  }
}
