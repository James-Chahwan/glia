import { Routes } from '@angular/router';
import { HomeComponent } from './home/home.component';
import { LoginComponent } from './login.component';
import { ProfileComponent } from './profile.component';
import { UserViewComponent } from './user-view.component';

export const routes: Routes = [
  { path: 'home', component: HomeComponent },
  { path: 'login', component: LoginComponent },
  { path: 'verify-email', component: LoginComponent },
  { path: 'profile', component: ProfileComponent },
  { path: 'user/:publicId', component: UserViewComponent },
  { path: '**', redirectTo: '/login' },
];
