import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';

import { ApiUrlBuilder } from './api-url-builder';

@Injectable({ providedIn: 'root' })
export class FriendService {
  constructor(private readonly http: HttpClient, private readonly urls: ApiUrlBuilder) {}

  accept(publicId: string) {
    return this.http.post(this.urls.buildApiUrl(`protected/friends/accept/${encodeURIComponent(publicId)}`), {});
  }

  list() {
    return this.http.get(this.urls.buildApiUrl('protected/friends'));
  }

  checkSession() {
    return this.http.get(this.sessionUrl());
  }

  download(url: string) {
    return this.http.get(url);
  }

  private sessionUrl(): string {
    return this.urls.buildApiUrl('protected/user/profile');
  }
}
