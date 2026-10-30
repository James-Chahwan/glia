local Animal = require("animal")

local Dog = setmetatable({}, { __index = Animal })
Dog.__index = Dog

function Dog.new(name)
  local self = Animal.new(name)
  return setmetatable(self, Dog)
end

function Dog:bark()
  return self:speak() .. " (woof)"
end

return Dog
