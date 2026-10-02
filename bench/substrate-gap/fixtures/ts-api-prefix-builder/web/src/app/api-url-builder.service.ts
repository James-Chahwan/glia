import { Injectable } from '@angular/core';
import { environment } from '../environments/environment';

@Injectable({ providedIn: 'root' })
export class ApiUrlBuilderService {
  private readonly config = { apiBaseUrl: environment.apiBaseUrl, apiPrefix: environment.apiPrefix };

  buildApiUrl(path: string): string {
    return buildApiUrlFrom(this.config, path);
  }

  buildApiRootUrl(path: string): string {
    return new URL(`/${path}`, this.config.apiBaseUrl).toString();
  }
}

export function buildApiUrlFrom(config: { apiBaseUrl: string; apiPrefix: string }, path: string): string {
  return new URL(`${config.apiPrefix}/${path}`, config.apiBaseUrl).toString();
}
