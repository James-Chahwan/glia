local calc = require("calc")

describe("calc", function()
  it("adds", function()
    assert.are.equal(3, calc.add(1, 2))
  end)
end)
