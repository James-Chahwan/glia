class Ctl
  include Import["user_service", "mailer"]

  def show(id) = user_service.find(id)

  def notify(msg) = mailer.deliver(msg)
end
