import { Repo } from "./repo";

export class SqlRepo implements Repo {
  get(id: string): string {
    return id;
  }
}
