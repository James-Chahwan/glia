import { Component, HostBinding, HostListener, Input } from '@angular/core';

@Component({
  selector: 'ks-sheet',
  standalone: true,
  template: '<div class="sheet" [class.open]="open"><ng-content></ng-content></div>',
})
export class SheetComponent {
  open = false;

  @HostListener('window:resize', ['$event'])
  onResize(): void { this.open = false; }

  @HostBinding('class.is-open')
  get isOpen(): boolean { return this.open; }

  @Input()
  set title(value: string) { this.open = value.length > 0; }

  close(): void { this.open = false; }
}
