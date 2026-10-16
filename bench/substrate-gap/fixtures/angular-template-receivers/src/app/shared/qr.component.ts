import { Component, ElementRef, Input, ViewChild } from '@angular/core';

@Component({ selector: 'ks-qr', standalone: true, template: '<canvas #qr></canvas>' })
export class QrComponent {
  @Input() set link(value: string) { this.value = value; }
  value = '';

  @ViewChild('qr')
  private set canvas(el: ElementRef<HTMLCanvasElement> | undefined) { this.drawn = !!el; }
  drawn = false;

  redraw(): void { this.drawn = false; }
}
