import Config

config :shop, ShopWeb.Endpoint, secret_key_base: System.fetch_env!("SECRET_KEY_BASE")
