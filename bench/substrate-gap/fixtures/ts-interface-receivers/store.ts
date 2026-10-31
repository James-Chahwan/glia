export interface Store {
  load(id: string): string;
  save: (v: string) => void;
}
