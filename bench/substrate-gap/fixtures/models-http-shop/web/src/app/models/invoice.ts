export type InvoiceStatus = 'draft' | 'sent' | 'paid';

export interface InvoiceItem {
  sku: string;
  qty: number;
  unitCents: number;
}

export interface Invoice {
  id: string;
  customerName: string;
  status: InvoiceStatus;
  items: InvoiceItem[];
  totalCents: number;
  couponCode?: string;
  issuedAt: string;
}

export interface NewInvoice {
  customerName: string;
  items: InvoiceItem[];
  couponCode?: string;
}
