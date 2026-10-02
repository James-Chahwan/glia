import { Component, signal } from '@angular/core';
import { Subject } from 'rxjs';

import { Ticker } from './ticker';

@Component({ selector: 'app-hero', template: '' })
export class HeroComponent {
  private readonly ticks = new Subject<number>();
  private readonly ticker = new Ticker();
  private rafId = 0;
  readonly label = signal('hero');

  ngAfterViewInit(): void {
    window.addEventListener('resize', this.onResize);
    this.ticks.subscribe(this.handleTick);
    requestAnimationFrame(this.render.bind(this));
    this.ticker.start(this.ticker.stop);
    console.info(this.rafId, this.label);
  }

  ngOnDestroy(): void {
    window.removeEventListener('resize', this.onResize);
  }

  handleTick(n: number): void {
    this.rafId = n;
  }

  render(): void {
    this.handleTick(0);
  }

  private onResize = (): void => {
    this.render();
  };
}
