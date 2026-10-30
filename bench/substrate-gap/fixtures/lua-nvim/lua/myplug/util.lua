local M = {}

function M.merge(a, b)
  return vim.tbl_extend("force", a, b)
end

return M
