// DOM events: a native scroll listener and a synthetic resize.
export function attachScroll(container: HTMLElement, check: () => void) {
  container.addEventListener("scroll", check, { passive: true });
  window.dispatchEvent(new Event("resize"));
}
