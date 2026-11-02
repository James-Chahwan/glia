import { Routes } from '@angular/router';
import { HomeComponent, NotFoundComponent, ShellComponent } from './pages';

export const APP_ROUTES: Routes = [
  { path: '', component: HomeComponent },
  {
    path: 'app',
    component: ShellComponent,
    children: [
      {
        path: 'items',
        loadChildren: () => import('./features/items/routes').then(m => m.ITEM_ROUTES),
      },
    ],
  },
  { path: '**', component: NotFoundComponent },
];
