import { HttpClient } from '@angular/common/http';
import { Injectable } from '@angular/core';

export interface Plan {
  id: string;
  name: string;
  priceCents: number;
  trialDays: number;
}

@Injectable({ providedIn: 'root' })
export class PlanService {
  constructor(private http: HttpClient) {}

  plan(id: string) {
    return this.http.get<Plan>(`/api/plans/${id}`);
  }
}
