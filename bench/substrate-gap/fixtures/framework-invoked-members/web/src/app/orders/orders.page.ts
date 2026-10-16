import { Component, OnDestroy, OnInit } from '@angular/core';
import { OrderService } from '../core/order.service';

@Component({ selector: 'shop-orders', template: '<ul></ul>' })
export class OrdersPage implements OnInit, OnDestroy {
  rows: string[] = [];
  constructor(private orders: OrderService) {}
  ngOnInit(): void { this.rows = this.orders.list(); }
  ngOnDestroy(): void { this.rows = []; }
  ngOnDestroyLater(): void {}
}
