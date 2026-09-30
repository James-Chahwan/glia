require "mqtt"

MQTT::Client.connect("broker") do |c|
  c.publish("sensors/temp", "21")
end
