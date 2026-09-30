require "flipper"

class Checkout
  def variant(user)
    Flipper.enabled?(:"new-checkout", user) ? "new" : "legacy"
  end
end
