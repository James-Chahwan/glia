import { Component } from '@angular/core';
import { OrdersService } from './orders.service';

@Component({ selector: 'app-orders', template: '<ul></ul>' })
export class OrdersComponent {
  constructor(private orders: OrdersService) {}

  load() {
    return this.orders.list();
  }
}
