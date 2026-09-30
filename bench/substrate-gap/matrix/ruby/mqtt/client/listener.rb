require "mqtt"

MQTT::Client.connect("broker") do |c|
  c.subscribe("sensors/temp")
  c.get { |topic, message| puts message }
end
