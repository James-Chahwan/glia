import { TestBed } from '@angular/core/testing';
import { Svc } from './svc';

it('x', () => {
  const s = TestBed.inject(Svc);
  s.load();
});
