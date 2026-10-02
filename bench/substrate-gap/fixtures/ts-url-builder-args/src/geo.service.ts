import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';

@Injectable({ providedIn: 'root' })
export class GeoService {
  private readonly notificationsUrl = '/api/notifications';
  private draftUrl = '/api/drafts';

  constructor(private readonly http: HttpClient) {}

  async reverse(lat: number, lon: number) {
    const url = `https://nominatim.openstreetmap.org/reverse?lat=${lat}&lon=${lon}`;
    return fetch(url);
  }

  notifications() {
    return this.http.get(this.notificationsUrl);
  }

  drafts() {
    return this.http.get(this.draftUrl);
  }

  mutable() {
    let u = '/api/a';
    u = '/api/b';
    return this.http.get(u);
  }
}
