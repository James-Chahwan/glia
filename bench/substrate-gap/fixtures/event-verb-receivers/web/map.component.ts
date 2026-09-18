// Leaflet map and DOM listeners: library and DOM events, not an event bus.
import * as L from "leaflet";

export class MapComponent {
  private map!: L.Map;

  initMap(el: HTMLElement) {
    this.map = L.map(el);
    this.map.on("zoomend", () => this.refreshPins());
  }

  refreshPins() {
    const pin = document.createElement("div");
    pin.addEventListener("click", () => this.select());
  }

  select() {}
}
