import { Component } from '@angular/core';

@Component({ selector: 'app-hero', template: '<div></div>' })
export class HeroComponent {
  private rafId = 0;
  private readonly label = 'hero';

  ngAfterViewInit(): void {
    this.applyColor();
    window.matchMedia('(prefers-reduced-motion: reduce)').addEventListener('change', this.handleMotionChange);
  }

  stopAnimation(): void {
    this.rafId = 0;
  }

  renderFrame(): void {}

  persist(): void {}

  private applyColor = (): void => {
    this.renderFrame();
  };

  private handleMotionChange = (event: MediaQueryListEvent): void => {
    if (event.matches) {
      this.stopAnimation();
    }
  };

  onSave = function (this: HeroComponent): void {
    this.persist();
  };
}
