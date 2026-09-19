import { Injectable } from "@angular/core";
import { AuthService } from "@core/auth.service";

export class Page {
  constructor(private auth: AuthService) {}
}
