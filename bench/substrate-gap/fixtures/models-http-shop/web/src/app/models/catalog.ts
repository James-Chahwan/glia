export interface Article {
  id: string;
  title: string;
  priceCents: number;
  available: boolean;
}

export interface Listing {
  id: string;
  title: string;
  priceCents: number;
  sellerName?: string;
}

export interface Summary {
  total: number;
  count: number;
}
