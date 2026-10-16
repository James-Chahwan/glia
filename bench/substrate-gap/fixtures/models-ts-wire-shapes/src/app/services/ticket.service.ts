import { HttpClient } from '@angular/common/http';
import { Injectable } from '@angular/core';
import { environment } from '../../environments/environment';
import { NewTicket, Ticket } from '../models/ticket';

@Injectable({ providedIn: 'root' })
export class TicketService {
  constructor(private http: HttpClient) {}

  list() {
    return this.http.get<Ticket[]>(`${environment.apiUrl}/tickets`);
  }

  open(t: NewTicket) {
    return this.http.post<Ticket>(`${environment.apiUrl}/tickets`, t);
  }

  close(id: string, reason: string) {
    return this.http.patch<Ticket | null>(`${environment.apiUrl}/tickets/${id}`, { state: 'closed', reason });
  }

  board() {
    return this.http.get<{ tickets: Ticket[]; total: number }>(`${environment.apiUrl}/board`);
  }
}
