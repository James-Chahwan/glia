import { Base, IFoo } from "./base";

export class Child extends Base implements IFoo {
  run(): void {}
}
