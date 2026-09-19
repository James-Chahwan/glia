#!/usr/bin/env python3
"""pyo3 surface, py/src/pages.rs (LD.2, LA.6e handoff): `page_flow`
returns a native dict {pages, links, dead, unlinked}.
Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, rg

ROUTES = ("import { Routes } from '@angular/router';\n"
          "import { HomeComponent } from './home.component';\n\n"
          "export const routes: Routes = [\n  { path: 'home', component: HomeComponent },\n"
          "  { path: '**', redirectTo: '/home' },\n];\n")
HOME = ("import { Component } from '@angular/core';\nimport { Router } from '@angular/router';\n\n"
        "@Component({ selector: 'app-home', template: '<p>home</p>' })\n"
        "export class HomeComponent {\n  constructor(private router: Router) {}\n"
        "  go() {\n    this.router.navigateByUrl('/gone');\n  }\n}\n")


def main() -> int:
    c = Checks("pages")
    with tempfile.TemporaryDirectory(prefix="glia-surface-pages-") as tmp:
        app = pathlib.Path(tmp) / "web" / "src" / "app"
        app.mkdir(parents=True)
        (app / "app.routes.ts").write_text(ROUTES)
        (app / "home.component.ts").write_text(HOME)
        f = rg.generate(str(pathlib.Path(tmp) / "web")).page_flow()
        c.check("page_flow -> dict", type(f) is dict, type(f))
        c.check("keys in field order", type(f) is dict and list(f) == ["pages", "links", "dead", "unlinked"],
                type(f) is dict and list(f))
        dead = f.get("dead", []) if type(f) is dict else []
        c.check("the dead link reaches Python", [d.get("link") for d in dead] == ["/gone"], dead)
        c.check("line is an int", all(type(d.get("from_line")) is int for d in dead), dead)
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
