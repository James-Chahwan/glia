export interface Offer {
  id: string;
}

export function offerLabel(o: Offer): string {
  return o.id;
}
