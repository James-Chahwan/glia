import { Component } from '@angular/core';

@Component({ selector: 'app-toggle', standalone: true, template: '<button></button>' })
export class Toggle {
  sync(on: boolean): void {}
  flip(): void {}
}
