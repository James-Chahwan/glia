export function drain(queue: string[]): number {
  return queue.splice(0).length;
}

setInterval(() => drain([]), 1000);
