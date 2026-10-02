import { Injectable, inject } from '@angular/core';
import { HttpClient } from '@angular/common/http';

@Injectable({ providedIn: 'root' })
export class TradesApi {
  private readonly http = inject(HttpClient);

  get(id: string) {
    return this.http.get(`/api/trades/${id}`);
  }

  patch(id: string, body: unknown) {
    return this.http.patch(`/api/trades/${id}`, body);
  }

  fetchRaw(url: string) {
    return this.http.get(url);
  }
}
