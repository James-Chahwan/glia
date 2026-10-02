export class Ticker {
  start(cb: () => void): void {
    cb();
  }

  stop(): void {}
}
