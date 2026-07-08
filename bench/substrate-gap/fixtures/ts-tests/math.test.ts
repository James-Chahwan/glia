// TESTS probe (Jest): the test targets add().
import { add } from "./math";

describe("add", () => {
  it("adds two numbers", () => {
    expect(add(1, 2)).toBe(3);
  });
});
