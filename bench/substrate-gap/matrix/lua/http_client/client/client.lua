local http = require("resty.http")

local function place_order()
  local httpc = http.new()
  return httpc:request_uri("http://orders.internal/orders", { method = "POST", body = "{}" })
end

return { place_order = place_order }
