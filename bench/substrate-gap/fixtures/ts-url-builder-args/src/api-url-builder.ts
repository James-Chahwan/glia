import { Injectable } from '@angular/core';

@Injectable({ providedIn: 'root' })
export class ApiUrlBuilder {
  buildApiUrl(path: string): string {
    return `/api/${path}`;
  }
}
