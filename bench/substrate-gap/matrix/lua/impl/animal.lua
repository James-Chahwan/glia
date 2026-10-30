local Animal = {}
Animal.__index = Animal

function Animal.new(name)
  local self = setmetatable({}, Animal)
  self.name = name
  return self
end

function Animal:speak()
  return self.name .. " makes a sound"
end

return Animal
