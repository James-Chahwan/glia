// Angular @Output: EventEmitter.emit(value) pushes a component output.
import { Component, EventEmitter, Output } from "@angular/core";

@Component({ selector: "app-avatar", template: "" })
export class AvatarComponent {
  @Output() picked = new EventEmitter<string>();

  pick(url: string) {
    this.picked.emit(url);
  }
}
