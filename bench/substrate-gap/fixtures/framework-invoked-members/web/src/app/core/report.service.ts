import { Injectable, OnDestroy } from '@angular/core';

/** Injected by nothing: its constructor and hook stay dead with it. */
@Injectable({ providedIn: 'root' })
export class ReportService implements OnDestroy {
  constructor() {}
  ngOnDestroy(): void {}
}
