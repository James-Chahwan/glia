local M = {}

function M.charge(n)
  return n * 2
end

-- Same name as main.lua's local: a bare `total` there must never bind this.
function M.total()
end

return M
