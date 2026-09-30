import Config

config :shop, Shop.Repo, url: System.get_env("DATABASE_URL")
