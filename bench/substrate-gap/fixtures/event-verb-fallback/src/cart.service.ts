import { Injectable } from "@angular/core";
import { Subject } from "rxjs";

@Injectable({ providedIn: "root" })
export class CartService {
  private itemsSubject = new Subject<string[]>();

  add(items: string[]) {
    this.itemsSubject.next(items);
  }
}
