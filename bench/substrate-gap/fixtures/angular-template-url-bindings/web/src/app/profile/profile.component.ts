import { Component } from '@angular/core';

@Component({
  selector: 'shop-profile',
  templateUrl: './profile.component.html',
})
export class ProfileComponent {
  name = '';
  save(): void { this.name = this.name.trim(); }
  reset(): void { this.name = ''; }
}
