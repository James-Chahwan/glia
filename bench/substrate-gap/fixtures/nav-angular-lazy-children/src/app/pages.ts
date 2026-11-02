import { Component } from '@angular/core';

@Component({ selector: 'app-home', template: '<h1>Home</h1>' })
export class HomeComponent {}

@Component({ selector: 'app-shell', template: '<router-outlet></router-outlet>' })
export class ShellComponent {}

@Component({ selector: 'app-not-found', template: '<h1>Not found</h1>' })
export class NotFoundComponent {}
