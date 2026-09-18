import { Routes } from '@angular/router';
import { AdminShellComponent } from './admin-shell.component';
import { AdminUsersComponent } from './admin-users.component';
import { HomeComponent } from './home.component';
import { LoginComponent } from './login.component';

export const routes: Routes = [
  { path: 'admin', component: AdminShellComponent, children: [
      { path: 'users', component: AdminUsersComponent },
  ] },
  { path: 'home', component: HomeComponent },
  { path: 'login', component: LoginComponent },
  { path: 'chat', redirectTo: '/home', pathMatch: 'full' },
  { path: 'profile', loadComponent: () => import('./profile.component').then(m => m.ProfileComponent) },
  { path: '**', redirectTo: '/login' },
];
