defprotocol MyApp.Exception do
  @doc "The HTTP status an exception maps to."
  def status(exception)
end
