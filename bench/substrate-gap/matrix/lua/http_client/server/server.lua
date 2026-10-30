local lapis = require("lapis")
local app = lapis.Application()

local function show_user(self)
  return { json = { id = self.params.id } }
end

app:get("/users/:id", show_user)

app:post("create_order", "/orders", function(self)
  return { status = 201 }
end)

return app
