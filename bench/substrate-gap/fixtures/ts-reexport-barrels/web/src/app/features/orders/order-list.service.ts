import { Injectable } from '@angular/core';
import { Order, orderTotal } from '@app/core';
import { badgeLabel } from '@app/shared';

@Injectable({ providedIn: 'root' })
export class OrderListService {
  total(orders: Order[]): number { return orderTotal(orders); }
  label(n: number): string { return badgeLabel(n); }
}
