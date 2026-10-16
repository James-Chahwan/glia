import { Injectable, OnDestroy } from '@angular/core';

@Injectable({ providedIn: 'root' })
export class OrderService implements OnDestroy {
  private timer = 0;
  constructor() { this.timer = 1; }
  list(): string[] { return []; }
  ngOnDestroy(): void { this.timer = 0; }
}
