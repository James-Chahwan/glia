import { Component, inject } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { CountriesService } from '../core/countries.service';

@Component({ selector: 'app-register', standalone: true, template: '<p></p>' })
export class RegisterPage {
  readonly countries = toSignal(inject(CountriesService).list(), { initialValue: CountriesService.FALLBACK });
}
