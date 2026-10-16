import { Routes } from '@angular/router';
import { authGuard } from './core/guards/auth.guard';

export const routes: Routes = [
  { path: 'trades', canActivate: [authGuard], loadComponent: () => import('./home/home.page').then((m) => m.HomePage) },
];
