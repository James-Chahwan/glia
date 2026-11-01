export function consume(batch: string[]): number {
  return batch.length;
}

setInterval(() => consume([]), 500);
