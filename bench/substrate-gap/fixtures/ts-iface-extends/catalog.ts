export interface Readable {
  read(id: string): string;
}

export interface Catalog extends Readable {
  search(q: string): string[];
}

export class PgCatalog implements Catalog {
  read(id: string): string { return id; }
  search(q: string): string[] { return [q]; }
}
