import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';

import { ApiUrlBuilderService } from './api-url-builder.service';

@Injectable({ providedIn: 'root' })
export class FriendService {
  constructor(private readonly http: HttpClient, private readonly apiUrlBuilder: ApiUrlBuilderService) {}

  list() {
    return this.http.get(this.apiUrlBuilder.buildApiUrl('protected/friends'));
  }

  accept(publicId: string) {
    return this.http.post(this.apiUrlBuilder.buildApiUrl(`protected/friends/accept/${publicId}`), {});
  }

  health() {
    return this.http.get(this.apiUrlBuilder.buildApiRootUrl('healthz'));
  }
}
