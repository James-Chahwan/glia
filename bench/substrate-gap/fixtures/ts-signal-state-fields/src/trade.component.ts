import { Component, computed, effect, inject, input, signal } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { toSignal } from '@angular/core/rxjs-interop';
import { BehaviorSubject } from 'rxjs';

import { CountriesService } from './countries.service';

@Component({ selector: 'app-trade', template: '' })
export class TradeComponent {
  private readonly http = inject(HttpClient);
  private readonly countriesApi = inject(CountriesService);
  private readonly subject = new BehaviorSubject<number>(0);
  private rafId = 0;

  readonly rows = input.required<string[]>();
  readonly page = signal(1);
  readonly total = computed(() => this.rows().length * this.page());
  readonly isLong = computed(() => this.longest(this.rows()) > 10);
  readonly countries = toSignal(this.countriesApi.list(), { initialValue: [] });
  readonly profile = toSignal(this.http.get<string>('/api/profile'));
  readonly count$ = this.subject.asObservable();
  private readonly logTotal = effect(() => this.log(this.total()));

  longest(rows: string[]): number {
    return Math.max(0, ...rows.map((r) => r.length));
  }

  log(n: number): void {
    console.info(n);
  }

  next(): void {
    this.page.set(this.page() + 1);
  }
}
