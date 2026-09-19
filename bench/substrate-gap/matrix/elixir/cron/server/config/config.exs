import Config

config :my_app, Oban,
  plugins: [
    {Oban.Plugins.Cron,
     crontab: [
       {"0 * * * *", MyApp.Workers.HourlyWorker},
       {"@daily", MyApp.Workers.DailyWorker}
     ]}
  ]
