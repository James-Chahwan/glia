import { Component, forwardRef } from '@angular/core';
import { ControlValueAccessor, NG_VALUE_ACCESSOR } from '@angular/forms';

@Component({
  selector: 'ks-amount-input',
  template: '<input />',
  providers: [{ provide: NG_VALUE_ACCESSOR, useExisting: forwardRef(() => AmountInputComponent), multi: true }],
})
export class AmountInputComponent implements ControlValueAccessor {
  private value = 0;
  private disabled = false;
  writeValue(v: number): void { this.value = v; }
  registerOnChange(_fn: (v: number) => void): void {}
  registerOnTouched(_fn: () => void): void {}
  setDisabledState(d: boolean): void { this.disabled = d; }
}
