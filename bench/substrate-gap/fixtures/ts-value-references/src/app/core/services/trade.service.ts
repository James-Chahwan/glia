import { Injectable, inject } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { environment } from '../../../environments/environment';

@Injectable({ providedIn: 'root' })
export class TradeService {
  private readonly http = inject(HttpClient);
  private get apiUrl() { return environment.apiUrl; }
  private get unused() { return 1; }

  list() {
    return this.http.get(`${this.apiUrl}/trades`);
  }
}
