import { HttpClient } from '@angular/common/http';
import { Injectable } from '@angular/core';
import { environment } from '../../environments/environment';
import { Article, Listing, Summary } from '../models/catalog';

@Injectable({ providedIn: 'root' })
export class CatalogService {
  constructor(private http: HttpClient) {}

  articles() {
    return this.http.get<Article[]>(`${environment.apiUrl}/articles`);
  }

  listing(id: string) {
    return this.http.get<Listing>(`${environment.apiUrl}/listings/${id}`);
  }

  listings() {
    return this.http.get<{ listings: Listing[] }>(`${environment.apiUrl}/listings`);
  }

  daily() {
    return this.http.get<Summary>(`${environment.apiUrl}/summary/daily`);
  }

  weekly() {
    return this.http.get<Summary>(`${environment.apiUrl}/summary/weekly`);
  }
}
