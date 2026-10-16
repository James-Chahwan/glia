export type OrderStatus = 'pending' | 'paid' | 'shipped' | 'cancelled';

export interface OrderLine {
  productId: string;
  quantity: number;
  unitPriceCents: number;
}

export interface Order {
  id: string;
  customerName: string;
  status: OrderStatus;
  lines: OrderLine[];
  totalCents: number;
  discountCode?: string;
  createdAt: string;
}

export interface NewOrder {
  customerName: string;
  lines: OrderLine[];
  discountCode?: string;
}
