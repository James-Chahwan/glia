/** The legacy hash router: not Angular's `Router`, which the guards inject. */
export class Router {
  get url(): string {
    return window.location.hash;
  }
}
