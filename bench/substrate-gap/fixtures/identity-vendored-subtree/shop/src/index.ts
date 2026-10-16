import { parseCents } from '../third_party/money/src/parse';

export function main(): void { const total = parseCents('19.99'); console.log('total', total); }
