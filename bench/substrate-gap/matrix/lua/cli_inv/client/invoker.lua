local function run_sync()
  os.execute("mytool sync")
end

return { run_sync = run_sync }
