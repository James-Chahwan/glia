import { Component, signal } from '@angular/core';

interface Offer { id: string; amount: number; note: string }

@Component({
  selector: 'ks-offer-list',
  standalone: true,
  host: { class: 'block', '(document:keydown.escape)': 'onEscape()', '[class.busy]': 'isBusy()' },
  template: `
    <header>
      <h2>Offers - refresh to reload</h2>
      <button type="button" (click)="refresh()" [disabled]="isBusy()">Refresh</button>
      <small>{{ row() }}</small>
    </header>
    @if (canEdit()) {
      <p>{{ summary(offers()) }}</p>
    }
    @for (entry of offers(); track trackById($index, entry)) {
      <ks-offer-row [offer]="entry" (remove)="remove(entry.id)"></ks-offer-row>
      <span>{{ entry.note }}</span>
    }
    @for (row of rows; track row) {
      <i>{{ row }}</i>
    }
  `,
})
export class OfferListComponent {
  readonly offers = signal<Offer[]>([]);
  rows: string[] = [];
  private busy = false;

  refresh(): void { this.busy = true; }
  isBusy(): boolean { return this.busy; }
  canEdit(): boolean { return true; }
  summary(list: Offer[]): string { return `${list.length} offers`; }
  trackById(_i: number, o: Offer): string { return o.id; }
  remove(id: string): void { this.offers.update((l) => l.filter((o) => o.id !== id)); }
  onEscape(): void { this.busy = false; }
  /** Called outside the second @for, whose loop variable shares its name. */
  row(): string { return `${this.rows.length}`; }
  /** Named like the first @for's loop variable, and only that variable is read. */
  entry(): string { return 'shadowed'; }
  /** Named like a word in the template's text: no binding reads it. */
  reload(): void {}
  exportCsv(): string { return ''; }
}
