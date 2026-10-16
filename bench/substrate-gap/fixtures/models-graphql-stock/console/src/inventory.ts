import { client } from './client';
import { LIST_ITEMS, ADJUST_STOCK } from './documents';
import { InventoryItem } from './types';

export async function emptyShelves(): Promise<InventoryItem[]> {
  const { data } = await client.query<{ items: InventoryItem[] }>({ query: LIST_ITEMS, variables: { first: 100 } });
  return data.items.filter((i) => i.onHand === 0);
}

export async function adjust(id: string, delta: number): Promise<number> {
  const { data } = await client.mutate<{ adjustStock: InventoryItem }>({ mutation: ADJUST_STOCK, variables: { id, delta } });
  return data!.adjustStock.onHand;
}
