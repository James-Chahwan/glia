import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { adminBase } from './api-urls';

@Injectable({ providedIn: 'root' })
export class SmrAdminService {
  private readonly base = adminBase();

  constructor(private http: HttpClient) {}

  list(status?: string) {
    const q = status ? `?status=${encodeURIComponent(status)}` : '';
    return this.http.get(`${this.base}/smr${q}`);
  }

  create(input: unknown) {
    return this.http.post(`${this.base}/smr`, input);
  }
}
