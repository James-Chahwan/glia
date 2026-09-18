// A class that IS a Node EventEmitter: `this.emit` is an in-process emit.
import { EventEmitter } from "events";

export class Uploader extends EventEmitter {
  finish(file: string) {
    this.emit("uploaded", file);
  }
}
