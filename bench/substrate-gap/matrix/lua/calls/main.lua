local lib = require("lib")

local Shop = {}

local function total(items)
  return #items
end

function Shop.checkout(items)
  local n = total(items)
  return lib.charge(n)
end

function Shop:refresh()
  self:reload()
end

function Shop:reload()
end

return Shop
