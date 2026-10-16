import { Component } from '@angular/core';

export interface Order { id: string; totalCents: number; status: string }

@Component({
  selector: 'shop-orders',
  templateUrl: './orders.page.html',
})
export class OrdersPage {
  orders: Order[] = [];

  cancel(order: Order): void { order.status = 'cancelled'; }
  pay(order: Order): void { order.status = 'paid'; }
  totalLabel(order: Order): string { return `$${(order.totalCents / 100).toFixed(2)}`; }
  isLocked(order: Order): boolean { return order.status !== 'pending'; }
  trackByOrder(_i: number, o: Order): string { return o.id; }
  archive(order: Order): void { order.status = 'archived'; }
}
