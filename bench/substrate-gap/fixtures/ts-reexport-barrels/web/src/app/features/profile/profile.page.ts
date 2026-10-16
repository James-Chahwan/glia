import { Component, OnInit } from '@angular/core';
import { User, UserService, Journal, fmt, displayName } from '@app/core';

@Component({ selector: 'app-profile', template: '<p>{{ title }}</p>' })
export class ProfilePage implements OnInit {
  title = '';
  constructor(private users: UserService, private journal: Journal) {}
  ngOnInit(): void { this.load('me'); }
  load(id: string): void {
    const user: User = this.users.find(id);
    this.journal.record('load');
    this.title = fmt.shout(user.email);
    this.title = displayName(user);
  }
}
