import { Component } from '@angular/core';
import { TimeAgoPipe } from '../shared/time-ago.pipe';

interface Item { urgency: number }

export function byUrgency(a: Item, b: Item): number { return b.urgency - a.urgency; }
export function byName(a: Item, b: Item): number { return a.urgency - b.urgency; }

function sortWith(byName: (a: Item, b: Item) => number, items: Item[]): Item[] {
  return [...items].sort(byName);
}

@Component({ selector: 'ks-home', standalone: true, imports: [TimeAgoPipe], template: '<p></p>' })
export class HomePage {
  items: Item[] = [];
  sorted(): Item[] { return [...this.items].sort(byUrgency); }
  sortedBy(): Item[] { return sortWith(byUrgency, this.items); }
}
