import { Injectable } from '@angular/core';
import { Observable, of } from 'rxjs';

@Injectable({ providedIn: 'root' })
export class CountriesService {
  static readonly FALLBACK: readonly string[] = ['PG', 'AU'];

  list(): Observable<readonly string[]> {
    return of(CountriesService.FALLBACK);
  }
}

/** Referenced only by its own members. */
export class LegacyCodes {
  static readonly ALL: readonly string[] = ['X'];

  static first(): string {
    return LegacyCodes.ALL[0];
  }
}
