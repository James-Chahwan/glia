import { Component, ViewContainerRef, contentChild, viewChild, viewChildren } from '@angular/core';
import { Chip } from '../shared/chip.component';
import { Dropzone } from '../shared/dropzone.component';
import { FilingForm } from '../shared/filing-form.component';
import { Toggle } from '../shared/toggle.component';

@Component({
  selector: 'app-filing',
  standalone: true,
  imports: [Chip, FilingForm, Toggle],
  template: '<app-filing-form /><app-toggle #sw /><app-chip /><app-dropzone #zone /><div #box></div><ng-template #slot /><ng-content />',
})
export class FilingPage {
  private readonly form = viewChild(FilingForm);
  private readonly sw = viewChild<Toggle>('sw');
  private readonly zone = contentChild.required(Dropzone);
  private readonly slot = viewChild(Dropzone, { read: ViewContainerRef });
  private readonly zoneRef = viewChild<Dropzone, ViewContainerRef>('zone', { read: ViewContainerRef });
  private readonly box = viewChild('box');
  private readonly chips = viewChildren(Chip);
  private readonly toggles = viewChildren(Toggle);

  save(): void {
    this.form()?.reset();
    this.sw()?.sync(true);
    this.zone().clear();
  }

  reload(): void {
    this.slot()?.clear();
    this.zoneRef()?.clear();
    this.box()?.clear();
    this.chips().filter((c) => c.active);
    this.toggles().forEach((t) => t.flip());
  }
}
