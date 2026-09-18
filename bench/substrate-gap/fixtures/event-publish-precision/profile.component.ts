// RxJS observable subscriptions are not an event bus.
import { Component } from "@angular/core";
import { ActivatedRoute } from "@angular/router";

@Component({ selector: "app-profile", template: "" })
export class ProfileComponent {
  constructor(private route: ActivatedRoute) {}

  ngOnInit() {
    this.route.params.subscribe((p) => this.load(p));
  }

  load(p: unknown) {}
}
