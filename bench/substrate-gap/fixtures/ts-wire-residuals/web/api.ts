import axios, { AxiosInstance } from 'axios';
import { User } from './models';

export class Api {
  constructor(private http: AxiosInstance = axios.create()) {}

  save(u: User) {
    return this.http.post('/users', u);
  }
}
