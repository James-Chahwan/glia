import { Injectable } from '@angular/core';
import { User } from '../models/user.model';
@Injectable({ providedIn: 'root' })
export class UserService {
  find(id: string): User { return new User(id, `${id}@example.com`); }
}
