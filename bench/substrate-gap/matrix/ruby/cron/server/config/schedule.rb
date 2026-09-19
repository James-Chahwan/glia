every 1.day, at: '4:30 am' do
  runner "Report.generate"
end

every '0 0 27-31 * *' do
  command "echo month-end"
end
