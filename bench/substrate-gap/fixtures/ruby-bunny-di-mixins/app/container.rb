class Container
  extend Dry::Container::Mixin

  register("user_service") { UserService.new }
  register("mailer") { SmtpMailer.new }
end
