import { Component, inject } from '@angular/core';

import { TradesApi } from './trades.api';

@Component({ selector: 'app-trade', template: '' })
export class TradePage {
  private readonly drafts = inject(TradesApi);
  private readonly timers = new Map<string, number>();
  private readonly selected: Set<string> = new Set<string>();

  constructor(private readonly trades: TradesApi) {}

  open(id: string) {
    return this.trades.get(id);
  }

  save(id: string) {
    return this.drafts.patch(id, { note: '' });
  }

  dismiss(id: string) {
    const t = this.timers.get(id);
    this.timers.delete(id);
    this.selected.delete(id);
    return t;
  }
}
