import { Pipe, PipeTransform } from '@angular/core';

/** Imported by no component. */
@Pipe({ name: 'initials', standalone: true })
export class InitialsPipe implements PipeTransform {
  transform(value: string): string { return value.slice(0, 1); }
}
