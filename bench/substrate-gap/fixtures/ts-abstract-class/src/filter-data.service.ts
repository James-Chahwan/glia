import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { Observable } from 'rxjs';

import { ProductService } from './product.service';

@Injectable()
export abstract class FilterDataService {
  constructor(protected http: HttpClient, private products: ProductService) {}

  protected abstract fetchMain(key: string): Observable<string[]>;

  load(key: string): Observable<string[]> {
    return this.fetchMain(key);
  }
}
