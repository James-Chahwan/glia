import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { environment } from '../environments/environment';

@Injectable({ providedIn: 'root' })
export class TradeService {
  constructor(private http: HttpClient) {}
  private get api() { return environment.apiUrl; }
  list() {
    return this.http.get(`${this.api}/trades`);
  }
}
