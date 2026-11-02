import { Routes } from '@angular/router';
import { ItemsPageComponent } from './items-page.component';
import { ItemPageComponent } from './item-page.component';

export const ITEM_ROUTES: Routes = [
  { path: '', component: ItemsPageComponent },
  { path: ':itemId', component: ItemPageComponent },
];
