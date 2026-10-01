import { Injectable } from '@angular/core';
import { Observable, of } from 'rxjs';

@Injectable({ providedIn: 'root' })
export class CountriesService {
  list(): Observable<string[]> {
    return of(['AU', 'PG']);
  }
}
