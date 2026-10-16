import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';

@Injectable({ providedIn: 'root' })
export class SmrService {
  constructor(private http: HttpClient) {}

  list(status: string) {
    const q = status ? `?status=${encodeURIComponent(status)}` : '';
    return this.http.get(`/admin/smr${q}`);
  }

  search(term: string) {
    return this.http.get(`/admin/smr${term ? '?q=' + term : ''}`);
  }
}
