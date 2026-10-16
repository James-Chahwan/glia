import { Injectable } from '@nestjs/common';
import { OnEvent } from '@nestjs/event-emitter';
import { BaseStore } from './base-store';

@Injectable()
export class StoresListener {
  constructor(private readonly store: BaseStore) {}

  @OnEvent('store.requested')
  onRequested(id: string): string {
    this.store.save(id);
    return this.store.load(id);
  }
}
